//! Provider, registry, and domain delivery adapters over durable state.
//!
//! `labrys-core::provider` plans scoped operations and gates destructive work
//! on named approval without I/O. This module performs the I/O: it routes one
//! [`ProviderOperation`] through an injected [`ProviderAdapter`], verifies one
//! OCI push through an injected [`OciRegistry`], and converges one hostname
//! through an injected [`DomainDeliveryAdapter`], persisting every outcome,
//! failure, usage unit, and audit event with redacted diagnostics.
//!
//! Boundaries:
//! - No adapter call happens without the controller/approval gate: destructive
//!   [`ProviderAction`]s are checked via [`ProviderOperation::require_approval`]
//!   before dispatch, and a denial records an attributable event with no side
//!   effect.
//! - Readiness comes only from a platform observation applied through
//!   [`PgResourceStore`]. An adapter `Accepted` keeps the resource in
//!   `provisioning`; agent completion claims are not a variant here.
//! - Credential values never reach SQL: only secret reference ids are stored,
//!   and every free-text diagnostic is redacted against caller secrets before
//!   INSERT, with a residual-plaintext guard.
//! - Registry promotion requires digest equality via
//!   [`labrys_core::verify_registry_digest`]; a mismatch is recorded with
//!   recovery guidance and never promotes.
//! - Traffic attaches only to a promoted healthy deployment behind propagated
//!   DNS and issued TLS; a certificate failure reports the failure and keeps
//!   traffic detached.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use labrys_core::{
    gate_registry_push, plan_traffic_attachment, sanitize_failure, verify_registry_digest,
    CertificateState, Deployment, DnsState, Domain, DomainDelivery, ExplicitApproval,
    ProviderAction, ProviderKind, ProviderObservation, ProviderOperation, RegistryArtifact,
    ResourceId, ResourcePhase,
};

use crate::error::{ControlPlaneError, Result};
use crate::mapping::{json_get_opt, to_value};
use crate::observability::{PgAuditLog, PgEventStore, PgUsageLedger};
use crate::repos::PgResourceStore;
use crate::{redact, worker};

use labrys_core::{
    Correlation, EventAction, EventActor, EventDraft, EventResource, EventResult, UsageRecord,
};

// ---------------------------------------------------------------------------
// Provider adapters
// ---------------------------------------------------------------------------

/// What one adapter call observed. Mirrors the core observation vocabulary so
/// the worker boundary can apply it without re-interpreting provider output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderExecution {
    Accepted { detail: String },
    Ready { detail: String },
    Degraded { reason: String },
    Failed { reason: String, recovery: String },
}

impl ProviderExecution {
    pub fn failed_redacted(reason: &str, secrets: &[&str]) -> Self {
        let (reason, recovery) = sanitize_failure(reason, secrets);
        Self::Failed { reason, recovery }
    }

    pub fn observation(&self) -> ProviderObservation {
        match self {
            Self::Accepted { detail } => ProviderObservation::Accepted {
                detail: detail.clone(),
            },
            Self::Ready { detail } => ProviderObservation::Ready {
                detail: detail.clone(),
            },
            Self::Degraded { reason } => ProviderObservation::Degraded {
                reason: reason.clone(),
            },
            Self::Failed { reason, recovery } => ProviderObservation::Failed {
                reason: reason.clone(),
                recovery: recovery.clone(),
            },
        }
    }

    pub fn target_phase(&self) -> ResourcePhase {
        self.observation().target_phase()
    }
}

/// Injected capability-provider execution boundary.
///
/// Implementations stand in for real PostgreSQL/auth/storage provisioning.
/// The durable [`ProviderRuntime`] below enforces approval, idempotency,
/// redaction, and observation persistence around whatever an adapter returns.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn key(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    async fn execute(&self, operation: &ProviderOperation) -> Result<ProviderExecution>;
}

/// Failure-injection script for one adapter call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectedFailure {
    Timeout,
    ProviderMismatch,
    CredentialLeak(String),
    Transient(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScriptedStep {
    Execution(ProviderExecution),
    Failure(InjectedFailure),
}

/// Local test adapter with a scripted outcome queue and failure injection.
///
/// When the script is exhausted it reports `Accepted`, so tests must script
/// every meaningful step. Timeouts surface as retryable failures; credential
/// leaks are sanitized with recovery guidance before they leave the adapter.
pub struct LocalTestAdapter {
    key: String,
    kind: ProviderKind,
    steps: Mutex<Vec<ScriptedStep>>,
    calls: Mutex<Vec<ProviderOperation>>,
}

impl LocalTestAdapter {
    pub fn new(key: impl Into<String>, kind: ProviderKind) -> Self {
        Self {
            key: key.into(),
            kind,
            steps: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn postgres_test() -> Self {
        Self::new("postgres.local-test", ProviderKind::Postgres)
    }

    pub fn auth_test() -> Self {
        Self::new("auth.local-test", ProviderKind::Auth)
    }

    pub fn file_storage_test() -> Self {
        Self::new("filestore.local-test", ProviderKind::FileStorage)
    }

    pub fn object_storage_test() -> Self {
        Self::new("objectstore.local-test", ProviderKind::ObjectStorage)
    }

    pub fn generic_test() -> Self {
        Self::new("generic.local-test", ProviderKind::Generic)
    }

    pub fn with_steps(self, steps: Vec<ProviderExecution>) -> Self {
        *self.steps.lock().expect("steps lock") =
            steps.into_iter().map(ScriptedStep::Execution).collect();
        self
    }

    pub fn with_failures(self, failures: Vec<InjectedFailure>) -> Self {
        *self.steps.lock().expect("steps lock") =
            failures.into_iter().map(ScriptedStep::Failure).collect();
        self
    }

    pub fn calls(&self) -> Vec<ProviderOperation> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait]
impl ProviderAdapter for LocalTestAdapter {
    fn key(&self) -> &str {
        &self.key
    }

    fn kind(&self) -> ProviderKind {
        self.kind
    }

    async fn execute(&self, operation: &ProviderOperation) -> Result<ProviderExecution> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(operation.clone());
        let step = self.steps.lock().expect("steps lock").pop();
        match step {
            None => Ok(ProviderExecution::Accepted {
                detail: format!("{} accepted {}", self.key, operation.resource_id),
            }),
            Some(ScriptedStep::Execution(outcome)) => Ok(outcome),
            Some(ScriptedStep::Failure(InjectedFailure::Timeout)) => {
                Ok(ProviderExecution::Failed {
                    reason: format!(
                        "provider {} timed out on {}",
                        self.key, operation.resource_id
                    ),
                    recovery: labrys_core::PROVIDER_FAILURE_RECOVERY.to_string(),
                })
            }
            Some(ScriptedStep::Failure(InjectedFailure::ProviderMismatch)) => {
                Err(ControlPlaneError::Provider(format!(
                    "provider {} does not supply capability '{}'",
                    self.key, operation.capability
                )))
            }
            Some(ScriptedStep::Failure(InjectedFailure::CredentialLeak(secret))) => {
                Ok(ProviderExecution::Failed {
                    reason: format!(
                        "provider error leaked credential {secret} for {}",
                        operation.resource_id
                    ),
                    recovery: labrys_core::PROVIDER_FAILURE_RECOVERY.to_string(),
                })
            }
            Some(ScriptedStep::Failure(InjectedFailure::Transient(reason))) => {
                Ok(ProviderExecution::Failed {
                    reason,
                    recovery: labrys_core::PROVIDER_FAILURE_RECOVERY.to_string(),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OCI registry boundary
// ---------------------------------------------------------------------------

/// Injected OCI registry boundary: push one verified artifact, pull by
/// reference. Local implementations verify digests without network I/O.
#[async_trait]
pub trait OciRegistry: Send + Sync {
    async fn push(&self, artifact: &RegistryArtifact) -> Result<String>;
    async fn pull(&self, reference: &str) -> Result<String>;
}

/// Digest-verifying local test registry with artifact retention.
///
/// `push` gates on [`gate_registry_push`], then either reports the recorded
/// digest (verified) or a caller-configured wrong digest (mismatch test). The
/// reported digest is always verified with [`verify_registry_digest`] before
/// it leaves the adapter, so a mismatch becomes a recorded error with
/// recovery guidance instead of a promotion.
pub struct LocalTestRegistry {
    retained: Mutex<HashMap<String, RegistryArtifact>>,
    mismatch_digest: Mutex<Option<String>>,
    refused: Mutex<Vec<String>>,
}

impl LocalTestRegistry {
    pub fn new() -> Self {
        Self {
            retained: Mutex::new(HashMap::new()),
            mismatch_digest: Mutex::new(None),
            refused: Mutex::new(Vec::new()),
        }
    }

    /// Makes the next push report `digest` instead of the recorded one.
    pub fn with_mismatch(digest: impl Into<String>) -> Self {
        let registry = Self::new();
        *registry.mismatch_digest.lock().expect("lock") = Some(digest.into());
        registry
    }

    pub fn retained(&self, reference: &str) -> Option<RegistryArtifact> {
        self.retained.lock().expect("lock").get(reference).cloned()
    }
}

impl Default for LocalTestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OciRegistry for LocalTestRegistry {
    async fn push(&self, artifact: &RegistryArtifact) -> Result<String> {
        if let Err(err) = gate_registry_push(artifact) {
            self.refused
                .lock()
                .expect("lock")
                .push(artifact.reference.clone());
            return Err(ControlPlaneError::Registry(err.to_string()));
        }
        let reported = self
            .mismatch_digest
            .lock()
            .expect("lock")
            .clone()
            .unwrap_or_else(|| artifact.digest.clone());
        verify_registry_digest(&artifact.digest, &reported)
            .map_err(|err| ControlPlaneError::Registry(err.to_string()))?;
        self.retained
            .lock()
            .expect("lock")
            .insert(artifact.reference.clone(), artifact.clone());
        Ok(reported)
    }

    async fn pull(&self, reference: &str) -> Result<String> {
        self.retained
            .lock()
            .expect("lock")
            .get(reference)
            .map(|artifact| artifact.digest.clone())
            .ok_or_else(|| {
                ControlPlaneError::NotFound(format!("artifact {reference} not retained"))
            })
    }
}

// ---------------------------------------------------------------------------
// Domain delivery boundary
// ---------------------------------------------------------------------------

/// Injected DNS/TLS/traffic boundary for one hostname.
#[async_trait]
pub trait DomainDeliveryAdapter: Send + Sync {
    async fn ensure_dns(&self, hostname: &str) -> Result<DnsState>;
    async fn issue_certificate(&self, hostname: &str) -> Result<CertificateState>;
    async fn attach_traffic(
        &self,
        delivery: &mut DomainDelivery,
        domain: &Domain,
        deployment: &Deployment,
        approval: &ExplicitApproval,
    ) -> Result<()>;
}

/// Local test domain delivery with scripted DNS/TLS states.
pub struct LocalDomainDelivery {
    dns: Mutex<DnsState>,
    tls: Mutex<CertificateState>,
}

impl LocalDomainDelivery {
    pub fn healthy() -> Self {
        Self {
            dns: Mutex::new(DnsState::Propagated),
            tls: Mutex::new(CertificateState::Issued),
        }
    }

    pub fn with_dns(state: DnsState) -> Self {
        Self {
            dns: Mutex::new(state),
            tls: Mutex::new(CertificateState::Issued),
        }
    }

    pub fn with_tls(state: CertificateState) -> Self {
        Self {
            dns: Mutex::new(DnsState::Propagated),
            tls: Mutex::new(state),
        }
    }
}

#[async_trait]
impl DomainDeliveryAdapter for LocalDomainDelivery {
    async fn ensure_dns(&self, hostname: &str) -> Result<DnsState> {
        let state = self.dns.lock().expect("lock").clone();
        match state {
            DnsState::Failed { reason } => Err(ControlPlaneError::Domain(format!(
                "DNS for '{hostname}' failed: {reason}"
            ))),
            state => Ok(state),
        }
    }

    async fn issue_certificate(&self, hostname: &str) -> Result<CertificateState> {
        let state = self.tls.lock().expect("lock").clone();
        match state {
            CertificateState::Failed { reason } => Err(ControlPlaneError::Domain(format!(
                "certificate issuance failed for '{hostname}': {reason}. {}",
                labrys_core::CERTIFICATE_FAILURE_RECOVERY
            ))),
            state => Ok(state),
        }
    }

    async fn attach_traffic(
        &self,
        delivery: &mut DomainDelivery,
        domain: &Domain,
        deployment: &Deployment,
        approval: &ExplicitApproval,
    ) -> Result<()> {
        plan_traffic_attachment(delivery, domain, deployment, approval)
            .map_err(|err| ControlPlaneError::Domain(err.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Durable operation records
// ---------------------------------------------------------------------------

/// Durable record of one scoped provider operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOperationRecord {
    pub id: String,
    pub operation: ProviderOperation,
    pub phase: ResourcePhase,
    pub failure: String,
    pub usage_units: i64,
}

/// Durable record of one registry push with its digest evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryDeliveryRecord {
    pub id: String,
    pub application_id: String,
    pub reference: String,
    pub recorded_digest: String,
    pub reported_digest: String,
    pub revision_sha: String,
    pub status: String,
    pub detail: String,
}

/// Durable record of one hostname's DNS/TLS/traffic state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainDeliveryRecord {
    pub id: String,
    pub domain_id: String,
    pub hostname: String,
    pub deployment_id: Option<String>,
    pub dns: String,
    pub tls: String,
    pub traffic_attached: bool,
    pub detail: String,
}

/// Durable provider/registry/domain delivery runtime.
///
/// Persists operations, digest evidence, and domain state with redacted
/// diagnostics; applies platform readiness observations through
/// [`PgResourceStore`]; and emits attributable events, usage, and audit
/// entries through the worker boundary.
#[derive(Clone)]
pub struct ProviderRuntime {
    pool: PgPool,
    resources: PgResourceStore,
    events: PgEventStore,
    audit: PgAuditLog,
    usage: PgUsageLedger,
    secrets: Vec<String>,
}

impl ProviderRuntime {
    pub fn new(pool: PgPool) -> Self {
        Self::with_secrets(pool, Vec::new())
    }

    pub fn with_secrets(pool: PgPool, secrets: Vec<String>) -> Self {
        Self {
            resources: PgResourceStore::new(pool.clone()),
            events: PgEventStore::new(pool.clone()),
            audit: PgAuditLog::new(pool.clone()),
            usage: PgUsageLedger::new(pool.clone()),
            pool,
            secrets,
        }
    }

    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }

    /// Executes one scoped operation through `adapter`, enforcing the approval
    /// gate first. A denial records an attributable event and performs no
    /// provider call. Outcomes persist the operation, the platform observation,
    /// usage, and audit evidence; failures are redacted with recovery.
    pub async fn execute_operation(
        &self,
        adapter: &dyn ProviderAdapter,
        operation: &ProviderOperation,
        approval: Option<&ExplicitApproval>,
        at: DateTime<Utc>,
    ) -> Result<ProviderExecution> {
        if let Err(gate) = operation.require_approval(approval) {
            let reason = gate.to_string();
            self.record_operation(operation, ResourcePhase::Failed, &reason, 0, at)
                .await?;
            self.record_event(
                operation,
                EventAction::Reconciled,
                EventResult::Failed {
                    reason: reason.clone(),
                },
                format!(
                    "provider {} denied: no provider call occurred",
                    operation.resource_id
                ),
                at,
            )
            .await?;
            return Err(ControlPlaneError::Core(gate));
        }
        let outcome = match adapter.execute(operation).await {
            Ok(outcome) => outcome,
            Err(err) => ProviderExecution::failed_redacted(&err.to_string(), &self.secret_refs()),
        };
        let redacted = redact_execution(&outcome, &self.secret_refs());
        let phase = redacted.target_phase();
        let failure = match &redacted {
            ProviderExecution::Failed { reason, recovery } => {
                format!("{reason} | recovery: {recovery}")
            }
            ProviderExecution::Degraded { reason } => reason.clone(),
            _ => String::new(),
        };
        self.record_operation(operation, phase, &failure, 1, at)
            .await?;
        // Platform observation only: Accepted keeps provisioning, Ready marks
        // ready, failures record retryable evidence. Agent claims never arrive.
        match &redacted {
            ProviderExecution::Accepted { .. } => {
                let _ = self
                    .resources
                    .observe_phase(
                        &operation.resource_id,
                        ResourcePhase::Provisioning,
                        labrys_core::ObservationSource::ProviderCallback,
                        Some(format!("accepted by {}", adapter.key())),
                        at,
                    )
                    .await;
            }
            ProviderExecution::Ready { detail } => {
                let _ = self
                    .resources
                    .observe_phase(
                        &operation.resource_id,
                        ResourcePhase::Ready,
                        labrys_core::ObservationSource::ProviderCallback,
                        Some(detail.clone()),
                        at,
                    )
                    .await;
            }
            ProviderExecution::Degraded { reason } => {
                let _ = self
                    .resources
                    .observe_phase(
                        &operation.resource_id,
                        ResourcePhase::Degraded,
                        labrys_core::ObservationSource::ProviderCallback,
                        Some(reason.clone()),
                        at,
                    )
                    .await;
            }
            ProviderExecution::Failed { reason, .. } => {
                let _ = self
                    .resources
                    .record_failure(
                        &operation.resource_id,
                        reason,
                        &self.secret_refs(),
                        labrys_core::RetryPolicy::default(),
                        at,
                    )
                    .await;
            }
        }
        let (action, result) = match &redacted {
            ProviderExecution::Failed { reason, .. } => (
                EventAction::Reconciled,
                EventResult::Failed {
                    reason: reason.clone(),
                },
            ),
            _ => (EventAction::ResourceProvisioned, EventResult::Succeeded),
        };
        self.record_event(
            operation,
            action,
            result,
            redacted.observation().to_event_detail(),
            at,
        )
        .await?;
        self.usage
            .record(&UsageRecord {
                application_id: operation.application_id,
                environment_id: operation.environment_id.unwrap_or_default(),
                period_start: at,
                period_end: at,
                cpu_millicore_seconds: 0,
                memory_mb_seconds: 0,
                build_seconds: 0,
                requests: 1,
                egress_bytes: 0,
            })
            .await?;
        Ok(redacted)
    }

    /// Pushes one artifact through `registry`, persisting the digest evidence.
    /// A mismatch is recorded with recovery guidance and never promotes.
    pub async fn push_artifact(
        &self,
        registry: &dyn OciRegistry,
        application_id: &str,
        artifact: &RegistryArtifact,
        at: DateTime<Utc>,
    ) -> Result<RegistryDeliveryRecord> {
        let id = format!("reg_{}", uuid::Uuid::new_v4().simple());
        match registry.push(artifact).await {
            Ok(reported) => {
                let record = RegistryDeliveryRecord {
                    id,
                    application_id: application_id.to_string(),
                    reference: artifact.reference.clone(),
                    recorded_digest: artifact.digest.clone(),
                    reported_digest: reported,
                    revision_sha: artifact.revision_sha.clone(),
                    status: "verified".to_string(),
                    detail: format!("digest verified for {}", artifact.reference),
                };
                self.save_registry(&record, at).await?;
                Ok(record)
            }
            Err(err) => {
                let detail = redact::redact(&err.to_string(), &self.secret_refs());
                let status = if detail.contains("digest mismatch") {
                    "mismatch"
                } else {
                    "refused"
                };
                let record = RegistryDeliveryRecord {
                    id,
                    application_id: application_id.to_string(),
                    reference: artifact.reference.clone(),
                    recorded_digest: artifact.digest.clone(),
                    reported_digest: String::new(),
                    revision_sha: artifact.revision_sha.clone(),
                    status: status.to_string(),
                    detail,
                };
                self.save_registry(&record, at).await?;
                Err(ControlPlaneError::Registry(record.detail.clone()))
            }
        }
    }

    /// Converges one hostname through `delivery_adapter`, persisting the
    /// separately observable DNS/TLS/traffic state. Certificate failures keep
    /// traffic detached and report the failure with recovery guidance.
    pub async fn converge_domain(
        &self,
        delivery_adapter: &dyn DomainDeliveryAdapter,
        delivery: &mut DomainDelivery,
        domain: &Domain,
        deployment: &Deployment,
        approval: &ExplicitApproval,
        at: DateTime<Utc>,
    ) -> Result<DomainDeliveryRecord> {
        match delivery_adapter.ensure_dns(&domain.hostname).await {
            Ok(state) => delivery.dns = state,
            Err(err) => {
                delivery.dns = DnsState::Failed {
                    reason: redact::redact(&err.to_string(), &self.secret_refs()),
                };
            }
        }
        match delivery_adapter.issue_certificate(&domain.hostname).await {
            Ok(state) => delivery.tls = state,
            Err(err) => {
                let reason = redact::redact(&err.to_string(), &self.secret_refs());
                delivery.report_certificate_failure(&reason, &[]);
            }
        }
        let attach = delivery_adapter
            .attach_traffic(delivery, domain, deployment, approval)
            .await;
        let record = DomainDeliveryRecord {
            id: format!("dom_{}", uuid::Uuid::new_v4().simple()),
            domain_id: delivery.domain_id.to_string(),
            hostname: domain.hostname.clone(),
            deployment_id: delivery.deployment_id.map(|id| id.to_string()),
            dns: dns_name(&delivery.dns).to_string(),
            tls: tls_name(&delivery.tls).to_string(),
            traffic_attached: delivery.traffic_attached,
            detail: delivery.detail.clone().unwrap_or_default(),
        };
        self.save_domain(&record, at).await?;
        attach?;
        Ok(record)
    }

    async fn record_operation(
        &self,
        operation: &ProviderOperation,
        phase: ResourcePhase,
        failure: &str,
        usage_units: i64,
        at: DateTime<Utc>,
    ) -> Result<ProviderOperationRecord> {
        let secrets = self.secret_refs();
        let failure = redact::redact_guard("provider_operations.failure", failure, &secrets)?;
        let id = format!("pop_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(
            r#"INSERT INTO provider_operations
                 (id, application_id, environment_id, resource_id, capability,
                  provider_key, provider_kind, action, idempotency_key, trace_id,
                  secret_refs, phase, failure, usage_units, created_at, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$15)
               ON CONFLICT (idempotency_key) DO NOTHING"#,
        )
        .bind(&id)
        .bind(operation.application_id.to_string())
        .bind(operation.environment_id.as_ref().map(|e| e.to_string()))
        .bind(operation.resource_id.to_string())
        .bind(&operation.capability)
        .bind(&operation.provider_key)
        .bind(provider_kind_name(operation.kind))
        .bind(action_label(operation.action))
        .bind(&operation.idempotency_key)
        .bind(&operation.trace_id)
        .bind(to_value(&operation.secret_refs)?)
        .bind(phase_name(phase))
        .bind(&failure)
        .bind(usage_units)
        .bind(at)
        .execute(&self.pool)
        .await?;
        self.get_operation(&operation.idempotency_key).await
    }

    async fn get_operation(&self, idempotency_key: &str) -> Result<ProviderOperationRecord> {
        let row = sqlx::query("SELECT * FROM provider_operations WHERE idempotency_key = $1")
            .bind(idempotency_key)
            .fetch_one(&self.pool)
            .await?;
        operation_from_row(&row)
    }

    pub async fn get_operation_by_key(
        &self,
        idempotency_key: &str,
    ) -> Result<ProviderOperationRecord> {
        self.get_operation(idempotency_key).await
    }

    async fn save_registry(
        &self,
        record: &RegistryDeliveryRecord,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let secrets = self.secret_refs();
        let detail = redact::redact_guard("registry_deliveries.detail", &record.detail, &secrets)?;
        sqlx::query(
            r#"INSERT INTO registry_deliveries
                 (id, application_id, reference, recorded_digest, reported_digest,
                  revision_sha, status, detail, created_at, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)"#,
        )
        .bind(&record.id)
        .bind(&record.application_id)
        .bind(&record.reference)
        .bind(&record.recorded_digest)
        .bind(&record.reported_digest)
        .bind(&record.revision_sha)
        .bind(&record.status)
        .bind(&detail)
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn save_domain(&self, record: &DomainDeliveryRecord, at: DateTime<Utc>) -> Result<()> {
        let secrets = self.secret_refs();
        let detail = redact::redact_guard("domain_deliveries.detail", &record.detail, &secrets)?;
        sqlx::query(
            r#"INSERT INTO domain_deliveries
                 (id, domain_id, hostname, deployment_id, dns, tls, traffic_attached,
                  detail, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
               ON CONFLICT (id) DO UPDATE SET
                 deployment_id = EXCLUDED.deployment_id,
                 dns = EXCLUDED.dns,
                 tls = EXCLUDED.tls,
                 traffic_attached = EXCLUDED.traffic_attached,
                 detail = EXCLUDED.detail,
                 updated_at = EXCLUDED.updated_at"#,
        )
        .bind(&record.id)
        .bind(&record.domain_id)
        .bind(&record.hostname)
        .bind(&record.deployment_id)
        .bind(&record.dns)
        .bind(&record.tls)
        .bind(record.traffic_attached)
        .bind(&detail)
        .bind(at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_event(
        &self,
        operation: &ProviderOperation,
        action: EventAction,
        result: EventResult,
        detail: String,
        at: DateTime<Utc>,
    ) -> Result<()> {
        let mut draft = EventDraft::new(
            at,
            EventActor::platform(format!("provider:{}", operation.provider_key)),
            Correlation {
                trace_id: operation.trace_id.clone(),
                session_id: None,
                application_id: operation.application_id.to_string().parse().ok(),
                environment_id: operation
                    .environment_id
                    .as_ref()
                    .and_then(|e| e.to_string().parse().ok()),
            },
            action,
            result,
        );
        draft.resource = Some(EventResource::new(
            "resource",
            operation.resource_id.to_string(),
        ));
        draft.after = Some(detail);
        self.events.append(&draft, &self.secret_refs()).await?;
        self.audit
            .append(
                at,
                &format!("provider:{}", operation.provider_key),
                &format!("provider:{}", action_label(operation.action)),
                &operation.resource_id.to_string(),
                &operation.idempotency_key,
                &[],
            )
            .await?;
        Ok(())
    }
}

fn redact_execution(outcome: &ProviderExecution, secrets: &[&str]) -> ProviderExecution {
    match outcome {
        ProviderExecution::Accepted { detail } => ProviderExecution::Accepted {
            detail: redact::redact(detail, secrets),
        },
        ProviderExecution::Ready { detail } => ProviderExecution::Ready {
            detail: redact::redact(detail, secrets),
        },
        ProviderExecution::Degraded { reason } => ProviderExecution::Degraded {
            reason: redact::redact(reason, secrets),
        },
        ProviderExecution::Failed { reason, recovery } => ProviderExecution::Failed {
            reason: redact::redact(reason, secrets),
            recovery: recovery.clone(),
        },
    }
}

fn action_label(action: ProviderAction) -> &'static str {
    match action {
        ProviderAction::Provision => "provision",
        ProviderAction::ReadinessCheck => "readiness-check",
        ProviderAction::Replace => "replace",
        ProviderAction::Migrate => "migrate",
        ProviderAction::Adopt => "adopt",
        ProviderAction::Delete => "delete",
    }
}

fn provider_kind_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Postgres => "postgres",
        ProviderKind::Auth => "auth",
        ProviderKind::FileStorage => "file-storage",
        ProviderKind::ObjectStorage => "object-storage",
        ProviderKind::Generic => "generic",
    }
}

fn phase_name(phase: ResourcePhase) -> &'static str {
    match phase {
        ResourcePhase::Requested => "requested",
        ResourcePhase::Provisioning => "provisioning",
        ResourcePhase::Ready => "ready",
        ResourcePhase::Degraded => "degraded",
        ResourcePhase::Failed => "failed",
        ResourcePhase::Deleting => "deleting",
        ResourcePhase::Deleted => "deleted",
    }
}

fn phase_from_name(name: &str) -> Result<ResourcePhase> {
    Ok(match name {
        "requested" => ResourcePhase::Requested,
        "provisioning" => ResourcePhase::Provisioning,
        "ready" => ResourcePhase::Ready,
        "degraded" => ResourcePhase::Degraded,
        "failed" => ResourcePhase::Failed,
        "deleting" => ResourcePhase::Deleting,
        "deleted" => ResourcePhase::Deleted,
        other => {
            return Err(ControlPlaneError::Mapping(format!(
                "unknown provider phase {other}"
            )))
        }
    })
}

fn dns_name(state: &DnsState) -> &'static str {
    match state {
        DnsState::Pending => "pending",
        DnsState::Propagated => "propagated",
        DnsState::Failed { .. } => "failed",
    }
}

fn tls_name(state: &CertificateState) -> &'static str {
    match state {
        CertificateState::Pending => "pending",
        CertificateState::Issued => "issued",
        CertificateState::Renewing => "renewing",
        CertificateState::Failed { .. } => "failed",
    }
}

fn operation_from_row(row: &sqlx::postgres::PgRow) -> Result<ProviderOperationRecord> {
    let id: String = row.try_get("id")?;
    let application_id: String = row.try_get("application_id")?;
    let environment_id: Option<String> = row.try_get("environment_id")?;
    let resource_id: String = row.try_get("resource_id")?;
    let capability: String = row.try_get("capability")?;
    let provider_key: String = row.try_get("provider_key")?;
    let provider_kind: String = row.try_get("provider_kind")?;
    let action: String = row.try_get("action")?;
    let idempotency_key: String = row.try_get("idempotency_key")?;
    let trace_id: String = row.try_get("trace_id")?;
    let secret_refs: Vec<String> = json_get_opt(row, "secret_refs")?.unwrap_or_default();
    let phase: String = row.try_get("phase")?;
    let operation = ProviderOperation {
        application_id: application_id.parse().map_err(|_| {
            ControlPlaneError::Mapping(format!("invalid application id {application_id}"))
        })?,
        environment_id: environment_id
            .map(|e| {
                e.parse()
                    .map_err(|_| ControlPlaneError::Mapping(format!("invalid environment id {e}")))
            })
            .transpose()?,
        resource_id: resource_id.parse().map_err(|_| {
            ControlPlaneError::Mapping(format!("invalid resource id {resource_id}"))
        })?,
        capability,
        provider_key,
        kind: match provider_kind.as_str() {
            "postgres" => ProviderKind::Postgres,
            "auth" => ProviderKind::Auth,
            "file-storage" => ProviderKind::FileStorage,
            "object-storage" => ProviderKind::ObjectStorage,
            _ => ProviderKind::Generic,
        },
        action: match action.as_str() {
            "provision" => ProviderAction::Provision,
            "readiness-check" => ProviderAction::ReadinessCheck,
            "replace" => ProviderAction::Replace,
            "migrate" => ProviderAction::Migrate,
            "adopt" => ProviderAction::Adopt,
            _ => ProviderAction::Delete,
        },
        idempotency_key,
        trace_id,
        secret_refs,
        attempt: 1,
    };
    let failure: String = row.try_get("failure")?;
    let usage_units: i64 = row.try_get("usage_units")?;
    Ok(ProviderOperationRecord {
        id,
        operation,
        phase: phase_from_name(&phase)?,
        failure,
        usage_units,
    })
}

// ---------------------------------------------------------------------------
// Worker wiring: provider jobs through the durable queue
// ---------------------------------------------------------------------------

/// A [`worker::JobDispatcher`] routing provision/update/delete jobs through
/// one [`ProviderAdapter`] per provider key, persisting observations through
/// [`ProviderRuntime`]. Unknown provider keys report a retryable failure with
/// recovery guidance instead of simulating success.
pub struct ProviderJobDispatcher {
    runtime: ProviderRuntime,
    adapters: HashMap<String, Arc<dyn ProviderAdapter>>,
}

impl ProviderJobDispatcher {
    pub fn new(runtime: ProviderRuntime) -> Self {
        Self {
            runtime,
            adapters: HashMap::new(),
        }
    }

    pub fn with_adapter(mut self, adapter: Arc<dyn ProviderAdapter>) -> Self {
        self.adapters.insert(adapter.key().to_string(), adapter);
        self
    }

    fn adapter_for(&self, job: &labrys_core::Job) -> Option<Arc<dyn ProviderAdapter>> {
        let target = job.target.to_lowercase();
        self.adapters
            .values()
            .find(|adapter| target.contains(&adapter.key().to_lowercase()))
            .cloned()
            .or_else(|| {
                self.adapters
                    .values()
                    .find(|adapter| adapter.kind() == ProviderKind::Generic)
                    .cloned()
            })
    }
}

#[async_trait]
impl worker::JobDispatcher for ProviderJobDispatcher {
    async fn dispatch(&self, job: &labrys_core::Job) -> Result<worker::DispatchOutcome> {
        let Some(adapter) = self.adapter_for(job) else {
            return Ok(worker::DispatchOutcome::Failed(format!(
                "no provider adapter for '{}'. {}",
                job.target,
                labrys_core::PROVIDER_FAILURE_RECOVERY
            )));
        };
        let resource_id: ResourceId = job.target.parse().unwrap_or_else(|_| ResourceId::new());
        let operation = ProviderOperation {
            application_id: labrys_core::ApplicationId::new(),
            environment_id: None,
            resource_id,
            capability: "database.postgres".to_string(),
            provider_key: adapter.key().to_string(),
            kind: adapter.kind(),
            action: match job.action {
                labrys_core::JobAction::Delete => ProviderAction::Delete,
                labrys_core::JobAction::Update => ProviderAction::Replace,
                _ => ProviderAction::Provision,
            },
            idempotency_key: job.idempotency_key.to_string(),
            trace_id: format!("job:{}", job.id),
            secret_refs: Vec::new(),
            attempt: job.attempts.max(1),
        };
        // Worker jobs carry no destructive approval: destructive actions are
        // denied here before any adapter call.
        let outcome = self
            .runtime
            .execute_operation(&*adapter, &operation, None, Utc::now())
            .await;
        Ok(match outcome {
            Ok(ProviderExecution::Ready { .. }) => worker::DispatchOutcome::Ready(resource_id),
            Ok(ProviderExecution::Degraded { .. }) => {
                worker::DispatchOutcome::Degraded(resource_id)
            }
            Ok(ProviderExecution::Accepted { .. }) => worker::DispatchOutcome::InProgress,
            Ok(ProviderExecution::Failed { reason, .. }) => worker::DispatchOutcome::Failed(reason),
            Err(err) => worker::DispatchOutcome::Failed(err.to_string()),
        })
    }
}
