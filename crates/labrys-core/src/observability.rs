//! Structured events, logs, health, usage, audit, and verification evidence.
//!
//! Covers requirement `verification-and-observability`: every agent and
//! platform action is attributable through a structured event carrying actor,
//! timestamp, application, environment, action, resource, before, after, and
//! result; logs are separated by source; and completion is decided by a
//! verifier that evaluates deterministic, build/test, health, schema, browser,
//! advisory (LLM), and human stages independently of any agent's completion
//! text — a later stage can never erase an earlier failure. Secret values are
//! redacted from every persisted surface while keeping a safe reference, the
//! failing resource, and the recovery path diagnosable, and the audit log is
//! hash-chained so tampering is detectable.
//!
//! This module is pure and deterministic; durable storage and OpenTelemetry
//! export are infrastructure built on these contracts.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::controller::redact_reason;
use crate::deployment::LogLevel;
use crate::error::{CoreError, Result};
use crate::ids::{AgentSessionId, ApplicationId, EnvironmentId};

/// Version of the observability contract vocabulary this crate implements.
pub const OBSERVABILITY_CONTRACT_VERSION: u32 = 1;

/// Redaction marker shared by every persisted surface.
pub const REDACTION_MARKER: &str = "[redacted]";

// ---------------------------------------------------------------------------
// Correlation and attribution
// ---------------------------------------------------------------------------

/// Correlation identifiers tying agent and platform actions together.
///
/// The same `trace_id` links an agent session's tool call to the platform
/// decision, the controller job, and the verifier evidence, so a diagnosis can
/// be assembled without any secret value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correlation {
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<AgentSessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<ApplicationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<EnvironmentId>,
}

impl Correlation {
    pub fn new(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            session_id: None,
            application_id: None,
            environment_id: None,
        }
    }

    pub fn with_session(mut self, session_id: AgentSessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn with_application(mut self, application_id: ApplicationId) -> Self {
        self.application_id = Some(application_id);
        self
    }

    pub fn with_environment(mut self, environment_id: EnvironmentId) -> Self {
        self.environment_id = Some(environment_id);
        self
    }
}

/// Who performed an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EventActor {
    /// An agent session; the session id keeps the action attributable.
    Agent {
        session_id: AgentSessionId,
        name: String,
    },
    /// A platform controller or subsystem.
    Platform { name: String },
    /// A named human operator.
    Human { name: String },
    /// An automated provider callback.
    Provider { name: String },
}

impl EventActor {
    pub fn agent(session_id: AgentSessionId, name: impl Into<String>) -> Self {
        Self::Agent {
            session_id,
            name: name.into(),
        }
    }

    pub fn platform(name: impl Into<String>) -> Self {
        Self::Platform { name: name.into() }
    }

    pub fn human(name: impl Into<String>) -> Self {
        Self::Human { name: name.into() }
    }

    /// Canonical actor label for queries.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Agent { .. } => "agent",
            Self::Platform { .. } => "platform",
            Self::Human { .. } => "human",
            Self::Provider { .. } => "provider",
        }
    }
}

/// Canonical action vocabulary carried by platform events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EventAction {
    CapabilityRequested,
    CapabilityDecision,
    ResourceProvisioned,
    SecretRotated,
    SecretReplaced,
    BuildStarted,
    BuildFinished,
    DeploymentRolledOut,
    DeploymentRolledBack,
    DomainChanged,
    HealthProbed,
    Reconciled,
    VerificationFinished,
    ApprovalRequested,
    ApprovalDecided,
}

impl EventAction {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::CapabilityRequested => "capability_requested",
            Self::CapabilityDecision => "capability_decision",
            Self::ResourceProvisioned => "resource_provisioned",
            Self::SecretRotated => "secret_rotated",
            Self::SecretReplaced => "secret_replaced",
            Self::BuildStarted => "build_started",
            Self::BuildFinished => "build_finished",
            Self::DeploymentRolledOut => "deployment_rolled_out",
            Self::DeploymentRolledBack => "deployment_rolled_back",
            Self::DomainChanged => "domain_changed",
            Self::HealthProbed => "health_probed",
            Self::Reconciled => "reconciled",
            Self::VerificationFinished => "verification_finished",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalDecided => "approval_decided",
        }
    }
}

/// The resource an action targeted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventResource {
    /// Kind key, e.g. `database.postgres` or `deployment`.
    pub kind: String,
    /// Stable identifier of the resource.
    pub id: String,
}

impl EventResource {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

/// Outcome of an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum EventResult {
    /// Request accepted; convergence still pending.
    Accepted,
    /// Decision denied the action.
    Denied,
    /// Action completed successfully.
    Succeeded,
    /// Action failed; the reason is always redacted.
    Failed { reason: String },
    /// Not yet decided.
    Pending,
}

impl EventResult {
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// One attributable platform event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformEvent {
    pub sequence: u64,
    pub at: DateTime<Utc>,
    pub actor: EventActor,
    pub correlation: Correlation,
    pub action: EventAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<EventResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub result: EventResult,
}

impl PlatformEvent {
    /// Renders the event for transcripts and dashboards. Never contains
    /// secret values; free-text fields are redacted on append.
    pub fn to_event_detail(&self) -> String {
        let resource = self
            .resource
            .as_ref()
            .map(|r| format!(" resource {}({})", r.id, r.kind))
            .unwrap_or_default();
        format!(
            "e{} {} {} via {}{} {:?}",
            self.sequence,
            self.actor.kind_label(),
            self.action.canonical_name(),
            self.correlation.trace_id,
            resource,
            self.result
        )
    }
}

/// A not-yet-persisted event; free-text fields are redacted on append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    pub at: DateTime<Utc>,
    pub actor: EventActor,
    pub correlation: Correlation,
    pub action: EventAction,
    pub resource: Option<EventResource>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub result: EventResult,
}

impl EventDraft {
    pub fn new(
        at: DateTime<Utc>,
        actor: EventActor,
        correlation: Correlation,
        action: EventAction,
        result: EventResult,
    ) -> Self {
        Self {
            at,
            actor,
            correlation,
            action,
            resource: None,
            before: None,
            after: None,
            result,
        }
    }

    pub fn on_resource(mut self, resource: EventResource) -> Self {
        self.resource = Some(resource);
        self
    }

    pub fn between(mut self, before: impl Into<String>, after: impl Into<String>) -> Self {
        self.before = Some(before.into());
        self.after = Some(after.into());
        self
    }
}

/// Append-only structured event store with correlation queries.
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<PlatformEvent>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a draft as the next sequence number, redacting every free-text
    /// field against the known secret values.
    pub fn append(&mut self, draft: EventDraft, secrets: &[&str]) -> &PlatformEvent {
        let sequence = self.events.len() as u64 + 1;
        let event = PlatformEvent {
            sequence,
            at: draft.at,
            actor: draft.actor,
            correlation: draft.correlation,
            action: draft.action,
            resource: draft.resource,
            before: draft.before.map(|b| redact_reason(&b, secrets)),
            after: draft.after.map(|a| redact_reason(&a, secrets)),
            result: match draft.result {
                EventResult::Failed { reason } => EventResult::Failed {
                    reason: redact_reason(&reason, secrets),
                },
                other => other,
            },
        };
        self.events.push(event);
        self.events.last().expect("just pushed")
    }

    pub fn events(&self) -> &[PlatformEvent] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Events for one application, oldest first.
    pub fn for_application(&self, application_id: ApplicationId) -> Vec<&PlatformEvent> {
        self.events
            .iter()
            .filter(|e| e.correlation.application_id == Some(application_id))
            .collect()
    }

    /// Events for one environment, oldest first.
    pub fn for_environment(&self, environment_id: EnvironmentId) -> Vec<&PlatformEvent> {
        self.events
            .iter()
            .filter(|e| e.correlation.environment_id == Some(environment_id))
            .collect()
    }

    /// Events for one agent session, oldest first.
    pub fn for_session(&self, session_id: AgentSessionId) -> Vec<&PlatformEvent> {
        self.events
            .iter()
            .filter(|e| e.correlation.session_id == Some(session_id))
            .collect()
    }

    /// Every event sharing a trace id, oldest first.
    pub fn for_trace(&self, trace_id: &str) -> Vec<&PlatformEvent> {
        self.events
            .iter()
            .filter(|e| e.correlation.trace_id == trace_id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Logs separated by source
// ---------------------------------------------------------------------------

/// Log stream origin. Streams stay separated so a noisy agent transcript can
/// never bury a provider or runtime failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LogSource {
    Agent,
    Build,
    Runtime,
    Deployment,
    Capability,
    Resource,
}

impl LogSource {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Build => "build",
            Self::Runtime => "runtime",
            Self::Deployment => "deployment",
            Self::Capability => "capability",
            Self::Resource => "resource",
        }
    }
}

/// One structured log line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    pub sequence: u64,
    pub at: DateTime<Utc>,
    pub source: LogSource,
    pub level: LogLevel,
    pub correlation: Correlation,
    pub message: String,
}

/// Append-only log store separated by source.
#[derive(Debug, Default)]
pub struct LogStore {
    entries: Vec<LogEntry>,
}

impl LogStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a line with the message redacted against known secrets.
    pub fn append(
        &mut self,
        source: LogSource,
        level: LogLevel,
        correlation: Correlation,
        message: &str,
        secrets: &[&str],
        at: DateTime<Utc>,
    ) -> &LogEntry {
        let sequence = self.entries.len() as u64 + 1;
        self.entries.push(LogEntry {
            sequence,
            at,
            source,
            level,
            correlation,
            message: redact_reason(message, secrets),
        });
        self.entries.last().expect("just pushed")
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// Only the lines belonging to one source stream.
    pub fn for_source(&self, source: LogSource) -> Vec<&LogEntry> {
        self.entries.iter().filter(|e| e.source == source).collect()
    }

    /// Error-level lines across every stream.
    pub fn errors(&self) -> Vec<&LogEntry> {
        self.entries
            .iter()
            .filter(|e| e.level == LogLevel::Error)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Health evidence and staleness
// ---------------------------------------------------------------------------

/// A point-in-time health observation of one target.
///
/// Health evidence expires: a snapshot older than its `ttl` no longer supports
/// any claim of readiness and reports [`HealthState::Unknown`](crate::HealthState)
/// through [`HealthSnapshot::effective_state`], so stale evidence can never be
/// mistaken for a current signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthSnapshot {
    pub target: String,
    pub state: crate::HealthState,
    pub observed_at: DateTime<Utc>,
    pub ttl: Duration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl HealthSnapshot {
    pub fn new(
        target: impl Into<String>,
        state: crate::HealthState,
        observed_at: DateTime<Utc>,
        ttl: Duration,
    ) -> Self {
        Self {
            target: target.into(),
            state,
            observed_at,
            ttl,
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// True once the snapshot is older than its ttl.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        now - self.observed_at > self.ttl
    }

    /// The state a consumer may act on: stale evidence reports Unknown.
    pub fn effective_state(&self, now: DateTime<Utc>) -> crate::HealthState {
        if self.is_stale(now) {
            crate::HealthState::Unknown
        } else {
            self.state
        }
    }
}

// ---------------------------------------------------------------------------
// Usage accounting
// ---------------------------------------------------------------------------

/// Usage metering for one accounting period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRecord {
    pub application_id: ApplicationId,
    pub environment_id: EnvironmentId,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub cpu_millicore_seconds: u64,
    pub memory_mb_seconds: u64,
    pub build_seconds: u64,
    pub requests: u64,
    pub egress_bytes: u64,
}

/// Per-application usage ledger.
#[derive(Debug, Default)]
pub struct UsageLedger {
    records: Vec<UsageRecord>,
}

impl UsageLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, record: UsageRecord) {
        self.records.push(record);
    }

    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }

    pub fn for_application(&self, application_id: ApplicationId) -> Vec<&UsageRecord> {
        self.records
            .iter()
            .filter(|r| r.application_id == application_id)
            .collect()
    }

    /// Totals across every recorded period for one application.
    pub fn totals(&self, application_id: ApplicationId) -> UsageTotals {
        let mut totals = UsageTotals::default();
        for record in self.for_application(application_id) {
            totals.cpu_millicore_seconds += record.cpu_millicore_seconds;
            totals.memory_mb_seconds += record.memory_mb_seconds;
            totals.build_seconds += record.build_seconds;
            totals.requests += record.requests;
            totals.egress_bytes += record.egress_bytes;
        }
        totals
    }
}

/// Aggregated usage for one application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub cpu_millicore_seconds: u64,
    pub memory_mb_seconds: u64,
    pub build_seconds: u64,
    pub requests: u64,
    pub egress_bytes: u64,
}

// ---------------------------------------------------------------------------
// Immutable audit trail
// ---------------------------------------------------------------------------

/// One hash-chained audit record. `hash` covers every field plus the previous
/// record's hash, so any edit breaks the chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    pub sequence: u64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: String,
    /// Redacted diagnostic; never contains secret values.
    pub detail: String,
    pub prev_hash: String,
    pub hash: String,
}

impl AuditRecord {
    /// Recomputes the digest over this record's content and chain link.
    pub fn compute_hash(&self) -> String {
        audit_digest(&[
            &self.sequence.to_string(),
            &self.at.to_rfc3339(),
            &self.actor,
            &self.action,
            &self.target,
            &self.detail,
            &self.prev_hash,
        ])
    }

    pub fn to_event_detail(&self) -> String {
        format!(
            "audit a{} {} {} -> {} by {} ({})",
            self.sequence, self.action, self.target, self.detail, self.actor, self.actor
        )
    }
}

fn audit_digest(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0x1f;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut second = hash ^ 0x51ed270b07a5ac65;
    for part in parts.iter().rev() {
        for byte in part.as_bytes() {
            second ^= u64::from(*byte);
            second = second.wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}{second:016x}")
}

/// Append-only, hash-chained audit log. There is no mutation or delete API;
/// [`AuditLog::verify`] proves the chain is intact after a reload.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditLog {
    records: Vec<AuditRecord>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Genesis hash for the first record's `prev_hash`.
    pub const GENESIS: &'static str =
        "0000000000000000000000000000000000000000000000000000000000000000";

    /// Appends an audit record, chaining it to the previous hash. The detail
    /// is redacted before it is stored.
    pub fn append(
        &mut self,
        at: DateTime<Utc>,
        actor: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        detail: &str,
        secrets: &[&str],
    ) -> &AuditRecord {
        let sequence = self.records.len() as u64 + 1;
        let prev_hash = self
            .records
            .last()
            .map(|r| r.hash.clone())
            .unwrap_or_else(|| Self::GENESIS.to_string());
        let mut record = AuditRecord {
            sequence,
            at,
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            detail: redact_reason(detail, secrets),
            prev_hash,
            hash: String::new(),
        };
        record.hash = record.compute_hash();
        self.records.push(record);
        self.records.last().expect("just pushed")
    }

    pub fn records(&self) -> &[AuditRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Reloads persisted records without recomputing hashes, so a tampered
    /// store can be detected by [`AuditLog::verify`].
    pub fn restore(records: Vec<AuditRecord>) -> Self {
        Self { records }
    }

    /// Verifies sequence, chain links, and per-record digests.
    pub fn verify(&self) -> Result<()> {
        let mut expected_prev = Self::GENESIS.to_string();
        for (index, record) in self.records.iter().enumerate() {
            if record.sequence != index as u64 + 1 {
                return Err(CoreError::AuditIntegrity(format!(
                    "sequence gap at index {index}: {}",
                    record.sequence
                )));
            }
            if record.prev_hash != expected_prev {
                return Err(CoreError::AuditIntegrity(format!(
                    "chain break at record {}",
                    record.sequence
                )));
            }
            if record.hash != record.compute_hash() {
                return Err(CoreError::AuditIntegrity(format!(
                    "digest mismatch at record {}",
                    record.sequence
                )));
            }
            expected_prev = record.hash.clone();
        }
        Ok(())
    }

    /// Renders the trail for a dashboard; details are stored redacted.
    pub fn to_event_detail(&self) -> String {
        self.records
            .iter()
            .map(|r| {
                format!(
                    "a{} {} {} -> {} by {}",
                    r.sequence, r.action, r.target, r.detail, r.actor
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// Verification evidence
// ---------------------------------------------------------------------------

/// Independent verifier stages. Advisory (LLM) is optional and never blocks;
/// every other stage is blocking when applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VerifierStage {
    /// Deterministic platform checks (schema of records, contract versions).
    Deterministic,
    /// Build and test outcome.
    BuildTest,
    /// Platform health probes.
    Health,
    /// Database/schema migration state.
    Schema,
    /// Real browser flow.
    Browser,
    /// Optional LLM advice; never authoritative.
    Advisory,
    /// Explicit human verification.
    Human,
}

impl VerifierStage {
    /// Advisory is the only non-blocking stage.
    pub fn is_blocking(self) -> bool {
        self != Self::Advisory
    }

    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::BuildTest => "build_test",
            Self::Health => "health",
            Self::Schema => "schema",
            Self::Browser => "browser",
            Self::Advisory => "advisory",
            Self::Human => "human",
        }
    }
}

/// Outcome of one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationEvidence {
    pub id: String,
    pub stage: VerifierStage,
    /// Name of the specific check that produced this evidence.
    pub check: String,
    pub passed: bool,
    /// Redacted summary; safe for transcripts.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<EventResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    pub at: DateTime<Utc>,
}

impl VerificationEvidence {
    pub fn new(
        id: impl Into<String>,
        stage: VerifierStage,
        check: impl Into<String>,
        passed: bool,
        summary: &str,
        secrets: &[&str],
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: id.into(),
            stage,
            check: check.into(),
            passed,
            summary: redact_reason(summary, secrets),
            resource: None,
            recovery: None,
            at,
        }
    }

    pub fn on_resource(mut self, resource: EventResource) -> Self {
        self.resource = Some(resource);
        self
    }

    pub fn with_recovery(mut self, recovery: impl Into<String>) -> Self {
        self.recovery = Some(recovery.into());
        self
    }

    /// Human-facing explanation of a failed check: what happened, where, and
    /// how to recover. Never contains secret values.
    pub fn explanation(&self) -> FailureExplanation {
        FailureExplanation {
            stage: self.stage,
            check: self.check.clone(),
            what: self.summary.clone(),
            resource: self
                .resource
                .as_ref()
                .map(|r| format!("{} ({})", r.id, r.kind)),
            recovery: self.recovery.clone().unwrap_or_else(|| {
                "no automated recovery path recorded; escalate to the owning team".to_string()
            }),
        }
    }

    pub fn to_event_detail(&self) -> String {
        format!(
            "evidence {} [{}] {} {}: {}",
            self.id,
            self.stage.canonical_name(),
            if self.passed { "PASS" } else { "FAIL" },
            self.check,
            self.summary
        )
    }
}

/// Failure explanation carrying the failing resource and recovery path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureExplanation {
    pub stage: VerifierStage,
    pub check: String,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    pub recovery: String,
}

impl FailureExplanation {
    pub fn to_event_detail(&self) -> String {
        let resource = self
            .resource
            .as_deref()
            .map(|r| format!(" resource: {r}"))
            .unwrap_or_default();
        format!(
            "[{}] {} failed: {what}{resource}; recovery: {recovery}",
            self.stage.canonical_name(),
            self.check,
            what = self.what,
            recovery = self.recovery
        )
    }
}

/// Retention policy for evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionPolicy {
    /// Evidence older than this is pruned.
    pub max_age: Duration,
    /// Minimum number of records always kept, whatever their age, so a recent
    /// failure is never lost to retention.
    pub keep_latest: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_age: Duration::days(90),
            keep_latest: 10,
        }
    }
}

impl RetentionPolicy {
    /// Prunes evidence older than `max_age`, always keeping the newest
    /// `keep_latest` records so a recent failure is never lost to retention.
    /// Returns the ids removed.
    pub fn prune(
        &self,
        evidence: &mut Vec<VerificationEvidence>,
        now: DateTime<Utc>,
    ) -> Vec<String> {
        let mut removed = Vec::new();
        if evidence.len() <= self.keep_latest {
            return removed;
        }
        let protected_start = evidence.len() - self.keep_latest;
        let kept: Vec<VerificationEvidence> = evidence
            .drain(..)
            .enumerate()
            .filter_map(|(index, record)| {
                let expired = now - record.at > self.max_age;
                if index < protected_start && expired {
                    removed.push(record.id.clone());
                    None
                } else {
                    Some(record)
                }
            })
            .collect();
        *evidence = kept;
        removed
    }
}

/// Store of verifier evidence with a retention policy.
#[derive(Debug, Default)]
pub struct EvidenceStore {
    evidence: Vec<VerificationEvidence>,
    policy: RetentionPolicy,
}

impl EvidenceStore {
    pub fn new(policy: RetentionPolicy) -> Self {
        Self {
            evidence: Vec::new(),
            policy,
        }
    }

    pub fn append(&mut self, evidence: VerificationEvidence) {
        self.evidence.push(evidence);
    }

    pub fn evidence(&self) -> &[VerificationEvidence] {
        &self.evidence
    }

    pub fn for_stage(&self, stage: VerifierStage) -> Vec<&VerificationEvidence> {
        self.evidence.iter().filter(|e| e.stage == stage).collect()
    }

    pub fn policy(&self) -> RetentionPolicy {
        self.policy
    }

    /// Applies the retention policy, returning the removed ids. Unresolved
    /// blocking failures are never pruned, so an operator can always diagnose
    /// the current breakage.
    pub fn retain(&mut self, now: DateTime<Utc>) -> Vec<String> {
        let has_failures = self
            .evidence
            .iter()
            .any(|e| !e.passed && e.stage.is_blocking());
        if !has_failures {
            return self.policy.prune(&mut self.evidence, now);
        }
        let mut removed = Vec::new();
        let kept: Vec<VerificationEvidence> = self
            .evidence
            .drain(..)
            .filter(|record| {
                let expired = now - record.at > self.policy.max_age;
                let is_failure = !record.passed && record.stage.is_blocking();
                if expired && !is_failure {
                    removed.push(record.id.clone());
                    false
                } else {
                    true
                }
            })
            .collect();
        self.evidence = kept;
        removed
    }
}

/// One verifier stage's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageResult {
    pub stage: VerifierStage,
    /// Whether the stage applies to this change at all.
    pub applicable: bool,
    pub evidence: Vec<VerificationEvidence>,
}

impl StageResult {
    pub fn not_applicable(stage: VerifierStage) -> Self {
        Self {
            stage,
            applicable: false,
            evidence: Vec::new(),
        }
    }

    pub fn passed(stage: VerifierStage, evidence: Vec<VerificationEvidence>) -> Self {
        Self {
            stage,
            applicable: true,
            evidence,
        }
    }

    pub fn failed(stage: VerifierStage, evidence: Vec<VerificationEvidence>) -> Self {
        Self::passed(stage, evidence)
    }

    /// True when every evidence record passed. An applicable stage with no
    /// evidence has nothing supporting it and never passes.
    pub fn is_passed(&self) -> bool {
        self.applicable && !self.evidence.is_empty() && self.evidence.iter().all(|e| e.passed)
    }

    pub fn is_failed(&self) -> bool {
        self.applicable && self.evidence.iter().any(|e| !e.passed)
    }
}

/// Verdict of an independent verification run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VerificationVerdict {
    /// At least one applicable blocking stage is missing evidence.
    Incomplete { missing: Vec<VerifierStage> },
    /// At least one blocking stage failed.
    Failed { first_failure: VerifierStage },
    /// Every applicable blocking stage passed on its own evidence.
    Passed,
}

impl VerificationVerdict {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// Independent verifier.
///
/// The agent's completion text is recorded for attribution only: it is never
/// an input to the verdict. Stage results are append-only, so a later stage
/// cannot erase an earlier failure, and a re-recorded stage that disagrees
/// with a stored failure is kept as a disagreement instead of overwriting it.
#[derive(Debug, Default)]
pub struct Verifier {
    stages: BTreeMap<VerifierStage, StageResult>,
    agent_claim: Option<String>,
    disagreements: Vec<String>,
}

impl Verifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records what the agent claimed. Never affects the verdict.
    pub fn record_agent_claim(&mut self, claim: impl Into<String>) {
        self.agent_claim = Some(claim.into());
    }

    pub fn agent_claim(&self) -> Option<&str> {
        self.agent_claim.as_deref()
    }

    /// Adds a stage result. A stored failure is never replaced by a later
    /// pass for the same stage; the conflict is recorded as a disagreement.
    pub fn add_stage(&mut self, result: StageResult) -> Result<()> {
        if let Some(existing) = self.stages.get(&result.stage) {
            if existing.is_failed() && result.is_passed() {
                self.disagreements.push(format!(
                    "stage {} already failed; later pass ignored",
                    result.stage.canonical_name()
                ));
                return Ok(());
            }
            if existing.is_passed() && result.is_failed() {
                self.disagreements.push(format!(
                    "stage {} already passed; later failure recorded",
                    result.stage.canonical_name()
                ));
                self.stages.insert(result.stage, result);
                return Ok(());
            }
            return Err(CoreError::Verification(format!(
                "stage {} already recorded",
                result.stage.canonical_name()
            )));
        }
        self.stages.insert(result.stage, result);
        Ok(())
    }

    pub fn stage(&self, stage: VerifierStage) -> Option<&StageResult> {
        self.stages.get(&stage)
    }

    /// Evidence recorded for one stage.
    pub fn for_stage_evidence(&self, stage: VerifierStage) -> Vec<&VerificationEvidence> {
        self.stages
            .get(&stage)
            .map(|result| result.evidence.iter().collect())
            .unwrap_or_default()
    }

    pub fn stages(&self) -> impl Iterator<Item = &StageResult> {
        self.stages.values()
    }

    pub fn disagreements(&self) -> &[String] {
        &self.disagreements
    }

    /// Applicable blocking stages that were never evaluated or have no
    /// evidence supporting them.
    pub fn missing_evidence(&self) -> Vec<VerifierStage> {
        let mut missing: Vec<VerifierStage> = Vec::new();
        for stage in [
            VerifierStage::Deterministic,
            VerifierStage::BuildTest,
            VerifierStage::Health,
            VerifierStage::Schema,
            VerifierStage::Browser,
            VerifierStage::Human,
        ] {
            match self.stages.get(&stage) {
                None => missing.push(stage),
                Some(result) if result.applicable && result.evidence.is_empty() => {
                    missing.push(stage)
                }
                _ => {}
            }
        }
        missing
    }

    /// The independent verdict. Completion requires evidence, so missing
    /// applicable stages block success even when every recorded stage passed.
    pub fn verdict(&self) -> VerificationVerdict {
        let first_failure = self
            .stages
            .values()
            .filter(|r| r.stage.is_blocking())
            .filter(|r| r.is_failed())
            .map(|r| r.stage)
            .min();
        if let Some(stage) = first_failure {
            return VerificationVerdict::Failed {
                first_failure: stage,
            };
        }
        let missing = self.missing_evidence();
        if !missing.is_empty() {
            return VerificationVerdict::Incomplete { missing };
        }
        VerificationVerdict::Passed
    }

    /// Explanations for every failed blocking stage, oldest stage first.
    pub fn failures(&self) -> Vec<FailureExplanation> {
        self.stages
            .values()
            .filter(|r| r.stage.is_blocking() && r.is_failed())
            .flat_map(|r| {
                r.evidence
                    .iter()
                    .filter(|e| !e.passed)
                    .map(|e| e.explanation())
            })
            .collect()
    }
}
