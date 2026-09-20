//! Redacting, durable persistence adapters for events, logs, the hash-chained
//! audit trail, verification evidence, and usage.
//!
//! Every free-text field is redacted against the caller-supplied known secret
//! values *before* SQL execution, and a defense-in-depth guard rejects any
//! residual plaintext secret so it can never reach a stored row.

use chrono::{DateTime, SubsecRound, Utc};
use sqlx::postgres::{PgConnection, PgExecutor};
use sqlx::Row;

use labrys_core::{
    AuditLog, AuditRecord, Correlation, EventAction, EventActor, EventDraft, EventResource,
    EventResult, EvidenceStore, LogEntry, LogLevel, LogSource, PlatformEvent, RetentionPolicy,
    UsageRecord, UsageTotals, VerificationEvidence, VerifierStage,
};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{json_get, json_get_opt, text_opt, to_value};
use crate::redact;

// Advisory-lock namespace for the audit appender, so concurrent appends get one
// unambiguous sequence and chain order.
const AUDIT_ADVISORY_KEY: i64 = 527_150_001;

/// Durable, queryable platform event store.
#[derive(Clone)]
pub struct PgEventStore {
    pool: sqlx::PgPool,
}

impl PgEventStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Appends an event, redacting free text before insert, and returns the
    /// stored event with its database-assigned sequence.
    pub async fn append(&self, draft: &EventDraft, secrets: &[&str]) -> Result<PlatformEvent> {
        let sequence = append_event(&self.pool, draft, secrets).await?;
        self.get(sequence).await
    }

    pub async fn get(&self, sequence: i64) -> Result<PlatformEvent> {
        let row = sqlx::query("SELECT * FROM events WHERE sequence = $1")
            .bind(sequence)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ControlPlaneError::NotFound(format!("event {sequence}")))?;
        event_from_row(&row)
    }

    pub async fn for_application(&self, application_id: &str) -> Result<Vec<PlatformEvent>> {
        let rows = sqlx::query("SELECT * FROM events WHERE application_id = $1 ORDER BY sequence")
            .bind(application_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn for_environment(&self, environment_id: &str) -> Result<Vec<PlatformEvent>> {
        let rows = sqlx::query("SELECT * FROM events WHERE environment_id = $1 ORDER BY sequence")
            .bind(environment_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn for_session(&self, session_id: &str) -> Result<Vec<PlatformEvent>> {
        let rows = sqlx::query("SELECT * FROM events WHERE session_id = $1 ORDER BY sequence")
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn for_trace(&self, trace_id: &str) -> Result<Vec<PlatformEvent>> {
        let rows = sqlx::query("SELECT * FROM events WHERE trace_id = $1 ORDER BY sequence")
            .bind(trace_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(event_from_row).collect()
    }
}

/// Inserts one redacted event on the given executor so it can share a
/// transaction with a desired-state mutation. Returns the assigned sequence.
pub(crate) async fn append_event<'e>(
    exec: impl PgExecutor<'e>,
    draft: &EventDraft,
    secrets: &[&str],
) -> Result<i64> {
    let before = redact::redact_guard(
        "event.before",
        draft.before.as_deref().unwrap_or(""),
        secrets,
    )?;
    let after = redact::redact_guard("event.after", draft.after.as_deref().unwrap_or(""), secrets)?;
    let before = draft.before.as_ref().map(|_| before);
    let after = draft.after.as_ref().map(|_| after);
    let result = match &draft.result {
        EventResult::Failed { reason } => EventResult::Failed {
            reason: redact::redact_guard("event.result", reason, secrets)?,
        },
        other => other.clone(),
    };
    let actor = to_value(&draft.actor)?;
    let result_value = to_value(&result)?;
    let row = sqlx::query(
        r#"INSERT INTO events
             (at, actor, trace_id, session_id, application_id, environment_id,
              action, resource_kind, resource_id, before_state, after_state, result)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
           RETURNING sequence"#,
    )
    .bind(draft.at)
    .bind(&actor)
    .bind(&draft.correlation.trace_id)
    .bind(draft.correlation.session_id.as_ref().map(|s| s.to_string()))
    .bind(
        draft
            .correlation
            .application_id
            .as_ref()
            .map(|s| s.to_string()),
    )
    .bind(
        draft
            .correlation
            .environment_id
            .as_ref()
            .map(|s| s.to_string()),
    )
    .bind(draft.action.canonical_name())
    .bind(draft.resource.as_ref().map(|r| r.kind.clone()))
    .bind(draft.resource.as_ref().map(|r| r.id.clone()))
    .bind(&before)
    .bind(&after)
    .bind(&result_value)
    .fetch_one(exec)
    .await?;
    let sequence: i64 = row.try_get("sequence")?;
    Ok(sequence)
}

fn event_from_row(row: &sqlx::postgres::PgRow) -> Result<PlatformEvent> {
    let sequence: i64 = row.try_get("sequence")?;
    let at: DateTime<Utc> = row.try_get("at")?;
    let actor: EventActor = json_get(row, "actor")?;
    let trace_id: String = row.try_get("trace_id")?;
    let action_name: String = row.try_get("action")?;
    let result: EventResult = json_get(row, "result")?;
    let correlation = Correlation {
        trace_id,
        session_id: text_opt(row, "session_id")?.map(parse_session),
        application_id: text_opt(row, "application_id")?.map(parse_application),
        environment_id: text_opt(row, "environment_id")?.map(parse_environment),
    };
    let resource = match (
        text_opt(row, "resource_kind")?,
        text_opt(row, "resource_id")?,
    ) {
        (Some(kind), Some(id)) => Some(EventResource::new(kind, id)),
        _ => None,
    };
    Ok(PlatformEvent {
        sequence: sequence as u64,
        at,
        actor,
        correlation,
        action: action_from_name(&action_name)?,
        resource,
        before: text_opt(row, "before_state")?,
        after: text_opt(row, "after_state")?,
        result,
    })
}

/// Durable, source-separated, redacted log store.
#[derive(Clone)]
pub struct PgLogStore {
    pool: sqlx::PgPool,
}

impl PgLogStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn append(
        &self,
        source: LogSource,
        level: LogLevel,
        correlation: &Correlation,
        message: &str,
        secrets: &[&str],
        at: DateTime<Utc>,
    ) -> Result<LogEntry> {
        let sequence =
            append_log(&self.pool, source, level, correlation, message, secrets, at).await?;
        let row = sqlx::query("SELECT * FROM logs WHERE sequence = $1")
            .bind(sequence)
            .fetch_one(&self.pool)
            .await?;
        log_from_row(&row)
    }

    pub async fn for_source(&self, source: LogSource) -> Result<Vec<LogEntry>> {
        let rows = sqlx::query("SELECT * FROM logs WHERE source = $1 ORDER BY sequence")
            .bind(source.canonical_name())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(log_from_row).collect()
    }

    pub async fn errors(&self) -> Result<Vec<LogEntry>> {
        let rows = sqlx::query("SELECT * FROM logs WHERE level = $1 ORDER BY sequence")
            .bind("error")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(log_from_row).collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn append_log<'e>(
    exec: impl PgExecutor<'e>,
    source: LogSource,
    level: LogLevel,
    correlation: &Correlation,
    message: &str,
    secrets: &[&str],
    at: DateTime<Utc>,
) -> Result<i64> {
    let message = redact::redact_guard("log.message", message, secrets)?;
    let row = sqlx::query(
        r#"INSERT INTO logs
             (at, source, level, trace_id, session_id, application_id, environment_id, message)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
           RETURNING sequence"#,
    )
    .bind(at)
    .bind(source.canonical_name())
    .bind(level_name(level))
    .bind(&correlation.trace_id)
    .bind(correlation.session_id.as_ref().map(|s| s.to_string()))
    .bind(correlation.application_id.as_ref().map(|s| s.to_string()))
    .bind(correlation.environment_id.as_ref().map(|s| s.to_string()))
    .bind(&message)
    .fetch_one(exec)
    .await?;
    let sequence: i64 = row.try_get("sequence")?;
    Ok(sequence)
}

fn log_from_row(row: &sqlx::postgres::PgRow) -> Result<LogEntry> {
    let sequence: i64 = row.try_get("sequence")?;
    let at: DateTime<Utc> = row.try_get("at")?;
    let source_name: String = row.try_get("source")?;
    let level_name: String = row.try_get("level")?;
    let trace_id: String = row.try_get("trace_id")?;
    Ok(LogEntry {
        sequence: sequence as u64,
        at,
        source: log_source_from_name(&source_name)?,
        level: log_level_from_name(&level_name)?,
        correlation: Correlation {
            trace_id,
            session_id: text_opt(row, "session_id")?.map(parse_session),
            application_id: text_opt(row, "application_id")?.map(parse_application),
            environment_id: text_opt(row, "environment_id")?.map(parse_environment),
        },
        message: row.try_get("message")?,
    })
}

/// Durable, append-only, hash-chained audit store.
#[derive(Clone)]
pub struct PgAuditLog {
    pool: sqlx::PgPool,
}

impl PgAuditLog {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Appends a redacted audit record, chaining it to the previous hash. The
    /// append runs inside a transaction holding a database-wide advisory lock so
    /// concurrent writers receive one unambiguous sequence and chain order.
    pub async fn append(
        &self,
        at: DateTime<Utc>,
        actor: &str,
        action: &str,
        target: &str,
        detail: &str,
        secrets: &[&str],
    ) -> Result<AuditRecord> {
        let mut tx = self.pool.begin().await?;
        let record = append_audit(&mut tx, at, actor, action, target, detail, secrets).await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Reloads every audit record in sequence order and returns a core
    /// [`AuditLog`] whose [`AuditLog::verify`] proves chain integrity.
    pub async fn load(&self) -> Result<AuditLog> {
        let rows = sqlx::query("SELECT * FROM audit_log ORDER BY sequence")
            .fetch_all(&self.pool)
            .await?;
        let records = rows
            .iter()
            .map(audit_from_row)
            .collect::<Result<Vec<_>>>()?;
        Ok(AuditLog::restore(records))
    }
}

/// Appends one audit record on the given connection. Must run inside a
/// transaction so the advisory lock spans the write and its commit.
pub(crate) async fn append_audit(
    conn: &mut PgConnection,
    at: DateTime<Utc>,
    actor: &str,
    action: &str,
    target: &str,
    detail: &str,
    secrets: &[&str],
) -> Result<AuditRecord> {
    // Round to the microsecond resolution PostgreSQL stores so the digest is
    // recomputable after a reload (the hash covers `at`).
    let at = at.round_subsecs(6);
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(AUDIT_ADVISORY_KEY)
        .execute(&mut *conn)
        .await?;
    let last: Option<(i64, String)> =
        sqlx::query("SELECT sequence, hash FROM audit_log ORDER BY sequence DESC LIMIT 1")
            .fetch_optional(&mut *conn)
            .await?
            .map(|row| {
                Ok::<(i64, String), ControlPlaneError>((
                    row.try_get::<i64, _>("sequence")?,
                    row.try_get::<String, _>("hash")?,
                ))
            })
            .transpose()?;
    let sequence = last.as_ref().map(|(s, _)| s + 1).unwrap_or(1);
    let prev_hash = last
        .as_ref()
        .map(|(_, h)| h.clone())
        .unwrap_or_else(|| AuditLog::GENESIS.to_string());
    let detail = redact::redact_guard("audit.detail", detail, secrets)?;
    let mut record = AuditRecord {
        sequence: sequence as u64,
        at,
        actor: actor.to_string(),
        action: action.to_string(),
        target: target.to_string(),
        detail,
        prev_hash,
        hash: String::new(),
    };
    record.hash = record.compute_hash();
    sqlx::query(
        r#"INSERT INTO audit_log (sequence, at, actor, action, target, detail, prev_hash, hash)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#,
    )
    .bind(sequence)
    .bind(at)
    .bind(&record.actor)
    .bind(&record.action)
    .bind(&record.target)
    .bind(&record.detail)
    .bind(&record.prev_hash)
    .bind(&record.hash)
    .execute(&mut *conn)
    .await?;
    Ok(record)
}

fn audit_from_row(row: &sqlx::postgres::PgRow) -> Result<AuditRecord> {
    let sequence: i64 = row.try_get("sequence")?;
    let at: DateTime<Utc> = row.try_get("at")?;
    Ok(AuditRecord {
        sequence: sequence as u64,
        at,
        actor: row.try_get("actor")?,
        action: row.try_get("action")?,
        target: row.try_get("target")?,
        detail: row.try_get("detail")?,
        prev_hash: row.try_get("prev_hash")?,
        hash: row.try_get("hash")?,
    })
}

/// Durable verification-evidence store with a retention policy.
#[derive(Clone)]
pub struct PgEvidenceStore {
    pool: sqlx::PgPool,
    policy: RetentionPolicy,
}

impl PgEvidenceStore {
    pub fn new(pool: sqlx::PgPool, policy: RetentionPolicy) -> Self {
        Self { pool, policy }
    }

    pub fn policy(&self) -> RetentionPolicy {
        self.policy
    }

    /// Appends evidence whose summary is redacted before insert.
    pub async fn append(
        &self,
        evidence: &VerificationEvidence,
        secrets: &[&str],
    ) -> Result<VerificationEvidence> {
        let summary = redact::redact_guard("evidence.summary", &evidence.summary, secrets)?;
        let resource = evidence.resource.as_ref().map(to_value).transpose()?;
        sqlx::query(
            r#"INSERT INTO evidence
                 (id, stage, check_name, passed, summary, resource, recovery, at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#,
        )
        .bind(&evidence.id)
        .bind(evidence.stage.canonical_name())
        .bind(&evidence.check)
        .bind(evidence.passed)
        .bind(&summary)
        .bind(&resource)
        .bind(&evidence.recovery)
        .bind(evidence.at)
        .execute(&self.pool)
        .await?;
        self.get(&evidence.id).await
    }

    pub async fn get(&self, id: &str) -> Result<VerificationEvidence> {
        let row = sqlx::query("SELECT * FROM evidence WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ControlPlaneError::NotFound(format!("evidence {id}")))?;
        evidence_from_row(&row)
    }

    pub async fn for_stage(&self, stage: VerifierStage) -> Result<Vec<VerificationEvidence>> {
        let rows = sqlx::query("SELECT * FROM evidence WHERE stage = $1 ORDER BY at")
            .bind(stage.canonical_name())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(evidence_from_row).collect()
    }

    pub async fn all(&self) -> Result<Vec<VerificationEvidence>> {
        let rows = sqlx::query("SELECT * FROM evidence ORDER BY at")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(evidence_from_row).collect()
    }

    /// Applies the retention policy durably: prunes expired, non-blocking
    /// evidence while always retaining unresolved blocking failures and the
    /// newest `keep_latest` records. Returns the removed ids. The selection
    /// reuses the tested `labrys-core` retention semantics, then deletes those
    /// rows in one transaction.
    pub async fn retain(&self, now: DateTime<Utc>) -> Result<Vec<String>> {
        let all = self.all().await?;
        let mut store = EvidenceStore::new(self.policy);
        for evidence in all {
            store.append(evidence);
        }
        let removed = store.retain(now);
        if removed.is_empty() {
            return Ok(removed);
        }
        let mut tx = self.pool.begin().await?;
        for id in &removed {
            sqlx::query("DELETE FROM evidence WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(removed)
    }
}

fn evidence_from_row(row: &sqlx::postgres::PgRow) -> Result<VerificationEvidence> {
    let stage_name: String = row.try_get("stage")?;
    let passed: bool = row.try_get("passed")?;
    let at: DateTime<Utc> = row.try_get("at")?;
    Ok(VerificationEvidence {
        id: row.try_get("id")?,
        stage: stage_from_name(&stage_name)?,
        check: row.try_get("check_name")?,
        passed,
        summary: row.try_get("summary")?,
        resource: json_get_opt(row, "resource")?,
        recovery: text_opt(row, "recovery")?,
        at,
    })
}

/// Durable per-application usage ledger.
#[derive(Clone)]
pub struct PgUsageLedger {
    pool: sqlx::PgPool,
}

impl PgUsageLedger {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    pub async fn record(&self, record: &UsageRecord) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO usage_records
                 (application_id, environment_id, period_start, period_end,
                  cpu_millicore_seconds, memory_mb_seconds, build_seconds, requests, egress_bytes)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
        )
        .bind(record.application_id.to_string())
        .bind(record.environment_id.to_string())
        .bind(record.period_start)
        .bind(record.period_end)
        .bind(record.cpu_millicore_seconds as i64)
        .bind(record.memory_mb_seconds as i64)
        .bind(record.build_seconds as i64)
        .bind(record.requests as i64)
        .bind(record.egress_bytes as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn for_application(&self, application_id: &str) -> Result<Vec<UsageRecord>> {
        let rows = sqlx::query("SELECT * FROM usage_records WHERE application_id = $1 ORDER BY id")
            .bind(application_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(usage_from_row).collect()
    }

    pub async fn totals(&self, application_id: &str) -> Result<UsageTotals> {
        let mut totals = UsageTotals::default();
        for record in self.for_application(application_id).await? {
            totals.cpu_millicore_seconds += record.cpu_millicore_seconds;
            totals.memory_mb_seconds += record.memory_mb_seconds;
            totals.build_seconds += record.build_seconds;
            totals.requests += record.requests;
            totals.egress_bytes += record.egress_bytes;
        }
        Ok(totals)
    }
}

fn usage_from_row(row: &sqlx::postgres::PgRow) -> Result<UsageRecord> {
    let application_id: String = row.try_get("application_id")?;
    let environment_id: String = row.try_get("environment_id")?;
    let period_start: DateTime<Utc> = row.try_get("period_start")?;
    let period_end: DateTime<Utc> = row.try_get("period_end")?;
    Ok(UsageRecord {
        application_id: parse_application(application_id),
        environment_id: parse_environment(environment_id),
        period_start,
        period_end,
        cpu_millicore_seconds: u64_from_i64(row.try_get("cpu_millicore_seconds")?),
        memory_mb_seconds: u64_from_i64(row.try_get("memory_mb_seconds")?),
        build_seconds: u64_from_i64(row.try_get("build_seconds")?),
        requests: u64_from_i64(row.try_get("requests")?),
        egress_bytes: u64_from_i64(row.try_get("egress_bytes")?),
    })
}

fn u64_from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

// --- name <-> enum helpers -------------------------------------------------

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn log_level_from_name(name: &str) -> Result<LogLevel> {
    Ok(match name {
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown log level {other}"
            )))
        }
    })
}

fn log_source_from_name(name: &str) -> Result<LogSource> {
    Ok(match name {
        "agent" => LogSource::Agent,
        "build" => LogSource::Build,
        "runtime" => LogSource::Runtime,
        "deployment" => LogSource::Deployment,
        "capability" => LogSource::Capability,
        "resource" => LogSource::Resource,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown log source {other}"
            )))
        }
    })
}

fn stage_from_name(name: &str) -> Result<VerifierStage> {
    Ok(match name {
        "deterministic" => VerifierStage::Deterministic,
        "build_test" => VerifierStage::BuildTest,
        "health" => VerifierStage::Health,
        "schema" => VerifierStage::Schema,
        "browser" => VerifierStage::Browser,
        "advisory" => VerifierStage::Advisory,
        "human" => VerifierStage::Human,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown verifier stage {other}"
            )))
        }
    })
}

fn action_from_name(name: &str) -> Result<EventAction> {
    Ok(match name {
        "capability_requested" => EventAction::CapabilityRequested,
        "capability_decision" => EventAction::CapabilityDecision,
        "resource_provisioned" => EventAction::ResourceProvisioned,
        "secret_rotated" => EventAction::SecretRotated,
        "secret_replaced" => EventAction::SecretReplaced,
        "build_started" => EventAction::BuildStarted,
        "build_finished" => EventAction::BuildFinished,
        "deployment_rolled_out" => EventAction::DeploymentRolledOut,
        "deployment_rolled_back" => EventAction::DeploymentRolledBack,
        "domain_changed" => EventAction::DomainChanged,
        "health_probed" => EventAction::HealthProbed,
        "reconciled" => EventAction::Reconciled,
        "verification_finished" => EventAction::VerificationFinished,
        "approval_requested" => EventAction::ApprovalRequested,
        "approval_decided" => EventAction::ApprovalDecided,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown event action {other}"
            )))
        }
    })
}

fn parse_session(value: String) -> labrys_core::AgentSessionId {
    value
        .parse()
        .unwrap_or_else(|_| labrys_core::AgentSessionId::default())
}

fn parse_application(value: String) -> labrys_core::ApplicationId {
    value
        .parse()
        .unwrap_or_else(|_| labrys_core::ApplicationId::default())
}

fn parse_environment(value: String) -> labrys_core::EnvironmentId {
    value
        .parse()
        .unwrap_or_else(|_| labrys_core::EnvironmentId::default())
}
