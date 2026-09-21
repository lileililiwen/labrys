//! Executable dispatcher selection for the control-plane daemon.
//!
//! The packaged daemon used to run the safe [`crate::worker::NoopDispatcher`]
//! unconditionally, so API-enqueued jobs never executed: deploys stayed
//! `InProgress`, previews never started, and provider provisioning never ran.
//! This module wires the library executors into the daemon process:
//!
//! - [`select_dispatcher`] probes the container runtime from process
//!   configuration and returns either an [`ExecutableDispatcher`] (real
//!   provider, container, and preview executors) or an
//!   [`UnavailableDispatcher`] that reports an environment blocker per
//!   execution stage and performs no execution at all.
//! - [`ExecutableDispatcher`] derives an [`ExecutionIdentity`] from each
//!   claimed job and carries it into the persisted platform event and the
//!   separated runtime log, with secret redaction unchanged. Readiness still
//!   comes only from platform observations; agent claims never reach this path.
//!
//! Boundaries:
//! - `labrys-core` stays I/O-free; this module only composes control-plane
//!   executors that already exist.
//! - A missing container runtime is an environment blocker with a recovery
//!   action, never a simulated pass. [`RuntimeMode::Docker`] additionally
//!   fails startup fast, because explicitly requesting a runtime that is not
//!   there is a misconfiguration.
//! - No dispatch-level verification evidence is fabricated: build/test and
//!   health evidence keep coming from the provider runtime and the preview
//!   manager, which observe real execution.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;

use labrys_core::{
    Correlation, EventAction, EventActor, EventDraft, EventResource, EventResult, Job, LogLevel,
    LogSource, RetentionPolicy,
};

use crate::config::{Config, RuntimeMode};
use crate::error::{ControlPlaneError, Result};
use crate::observability::{PgEventStore, PgLogStore};
use crate::oci_client::OciRegistryClient;
use crate::postgres_provider::PostgresProvisioner;
use crate::preview::{PreviewManager, WorkspaceRoot};
use crate::providers::{LocalTestAdapter, ProviderJobDispatcher, ProviderRuntime};
use crate::runtime::{ContainerExecutor, DockerExecutor, ExecutionIdentity, RuntimeAvailability};
use crate::storage_provider::{FileStorageProvisioner, FsBucketBackend, ObjectStorageProvisioner};
use crate::tls_dns::{OpensslTlsIssuer, RealDomainDelivery};
use crate::worker::{DispatchOutcome, JobDispatcher};

/// Actor attributed to dispatcher-stage events and logs.
pub const DISPATCHER_ACTOR: &str = "dispatch-runtime";

/// Recovery guidance attached to every runtime-blocker report.
pub const BLOCKER_RECOVERY: &str =
    "provision a container runtime (start Docker, or set LABRYS_DOCKER_BIN to its \
     binary and LABRYS_RUNTIME_MODE to auto/docker), then requeue the job; nothing \
     was executed and no pass was recorded";

/// Derives the execution identity for one claimed job.
///
/// The application part is the job target's leading segment (`{app}:deploy:{env}`
/// deploys carry the application first); the trace binds the job id so events,
/// logs, and evidence for this execution correlate. An empty target can never
/// yield an identity and fails the job with recovery instead of executing
/// blind.
pub fn identity_for_job(job: &Job) -> Result<ExecutionIdentity> {
    let target = job.target.trim();
    if target.is_empty() {
        return Err(ControlPlaneError::Execution(
            "job has an empty target: refusing to execute without an application identity; \
             re-enqueue with a valid target"
                .to_string(),
        ));
    }
    let application_part = target.split(':').next().unwrap_or(target);
    let mut identity = ExecutionIdentity::new(application_part, format!("job:{}", job.id))?;
    let mut segments = target.split(':');
    let _ = segments.next();
    if matches!(segments.next(), Some("deploy")) {
        if let Some(environment) = segments.next() {
            if !environment.trim().is_empty() {
                identity = identity.with_environment(environment);
            }
        }
    }
    Ok(identity)
}

/// Daemon dispatcher over the real provider, container, and preview executors.
///
/// Provider jobs execute through [`ProviderJobDispatcher`] (local test-double
/// adapters; destructive actions still require approval and are denied without
/// one). The bounded [`DockerExecutor`] and the health-gated
/// [`PreviewManager`] are constructed, availability-probed, and carried here so
/// every claimed job runs under the same execution posture the staging smoke
/// proves; per-job container work beyond the provider path needs a job payload
/// schema that does not exist yet, so dispatch records the identity-attributed
/// stage and delegates execution to the provider runtime.
#[derive(Clone)]
pub struct ExecutableDispatcher {
    provider: ProviderJobDispatcher,
    executor: DockerExecutor,
    previews: PreviewManager<DockerExecutor>,
    registry: Option<OciRegistryClient>,
    domain: RealDomainDelivery,
    availability: RuntimeAvailability,
    preview_ttl_secs: u64,
    events: PgEventStore,
    logs: PgLogStore,
    secrets: Vec<String>,
}

impl ExecutableDispatcher {
    /// Builds the real dispatcher from process configuration. Provider
    /// credentials come from the provider fields (`LABRYS_PROVIDER_*`); every
    /// configured surface runs its real adapter, and every unconfigured one
    /// keeps its local test double. No probe runs here; call
    /// [`select_dispatcher`] so startup reports the probed posture.
    pub fn build(pool: PgPool, config: &Config) -> Result<Self> {
        let executor = DockerExecutor::new(config.docker_bin.clone());
        let mut secrets = config.provider_secret_values();
        // Managed PostgreSQL when an admin URL is configured, test double
        // otherwise. The admin password is registered for redaction.
        let postgres: Arc<dyn crate::providers::ProviderAdapter> =
            match config.provider_postgres_url.as_deref() {
                Some(url) => {
                    let provisioner = PostgresProvisioner::new(url)?;
                    secrets.extend(provisioner.credential_secrets());
                    Arc::new(provisioner)
                }
                None => Arc::new(LocalTestAdapter::postgres_test()),
            };
        // Filesystem buckets are real without further configuration: the
        // backend is local directories plus process-memory keys.
        let storage_backend = FsBucketBackend::new(config.provider_storage_root.clone())?;
        let objectstore = ObjectStorageProvisioner::with_backend(storage_backend.clone());
        let filestore = FileStorageProvisioner::with_backend(storage_backend);
        let provider = ProviderJobDispatcher::new(ProviderRuntime::with_secrets(
            pool.clone(),
            secrets.clone(),
        ))
        .with_adapter(postgres)
        .with_adapter(Arc::new(LocalTestAdapter::auth_test()))
        .with_adapter(Arc::new(objectstore))
        .with_adapter(Arc::new(filestore))
        .with_adapter(Arc::new(LocalTestAdapter::generic_test()));
        // OCI delivery when a registry endpoint is configured.
        let registry = config
            .provider_registry_endpoint
            .clone()
            .map(|endpoint| {
                OciRegistryClient::new(
                    endpoint,
                    config.provider_registry_username.clone(),
                    config.provider_registry_password.clone(),
                )
            })
            .transpose()?;
        if let Some(client) = registry.as_ref() {
            secrets.extend(client.credential_secrets());
        }
        // DNS/TLS delivery is always constructed; issuance failures report
        // their environment blocker at call time, never a pass.
        let domain = RealDomainDelivery::new(OpensslTlsIssuer::new(
            std::path::PathBuf::from("openssl"),
            config.provider_tls_dir.clone(),
        )?);
        let previews = PreviewManager::new(
            pool.clone(),
            Arc::new(executor.clone()),
            WorkspaceRoot::new(config.workspace_root.clone())?,
            RetentionPolicy::default(),
        );
        Ok(Self {
            provider,
            executor,
            previews,
            registry,
            domain,
            availability: RuntimeAvailability::Unavailable {
                reason: "runtime has not been probed yet".to_string(),
            },
            preview_ttl_secs: config.preview_ttl_secs,
            events: PgEventStore::new(pool.clone()),
            logs: PgLogStore::new(pool),
            secrets,
        })
    }

    /// Records the probed runtime posture on this dispatcher.
    pub fn with_availability(mut self, availability: RuntimeAvailability) -> Self {
        self.availability = availability;
        self
    }

    /// Attaches known secret values for redaction of everything persisted.
    pub fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Swaps the provider dispatcher, so tests can inject scripted adapters
    /// without rebuilding the container and preview executors.
    pub fn with_provider(mut self, provider: ProviderJobDispatcher) -> Self {
        self.provider = provider;
        self
    }

    /// The container executor this dispatcher was built with.
    pub fn executor(&self) -> &DockerExecutor {
        &self.executor
    }

    /// The health-gated preview lifecycle over the daemon executor.
    pub fn preview_manager(&self) -> &PreviewManager<DockerExecutor> {
        &self.previews
    }

    /// The OCI registry client, when a registry endpoint is configured.
    pub fn registry_client(&self) -> Option<&OciRegistryClient> {
        self.registry.as_ref()
    }

    /// The real DNS/TLS domain delivery over the daemon TLS directory.
    pub fn domain_delivery(&self) -> &RealDomainDelivery {
        &self.domain
    }

    /// The probed runtime posture carried by this dispatcher.
    pub fn availability(&self) -> &RuntimeAvailability {
        &self.availability
    }

    /// Preview time-to-live (seconds) from process configuration.
    pub fn preview_ttl_secs(&self) -> u64 {
        self.preview_ttl_secs
    }

    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }

    async fn record_stage(
        &self,
        identity: &ExecutionIdentity,
        job: &Job,
        result: EventResult,
        level: LogLevel,
        detail: &str,
    ) -> Result<()> {
        let mut draft = EventDraft::new(
            Utc::now(),
            EventActor::platform(DISPATCHER_ACTOR),
            identity.correlation(),
            EventAction::Reconciled,
            result,
        );
        draft.resource = Some(EventResource::new("job", job.id.to_string()));
        draft.after = Some(detail.to_string());
        self.events.append(&draft, &self.secret_refs()).await?;
        self.logs
            .append(
                LogSource::Runtime,
                level,
                &identity.correlation(),
                detail,
                &self.secret_refs(),
                Utc::now(),
            )
            .await?;
        Ok(())
    }
}

#[async_trait]
impl JobDispatcher for ExecutableDispatcher {
    async fn dispatch(&self, job: &Job) -> Result<DispatchOutcome> {
        let identity = match identity_for_job(job) {
            Ok(identity) => identity,
            Err(err) => return Ok(DispatchOutcome::Failed(err.to_string())),
        };
        if let RuntimeAvailability::Unavailable { reason } = &self.availability {
            let detail = format!(
                "job {} on '{}' blocked: runtime unavailable ({reason}); recovery: {BLOCKER_RECOVERY}",
                job.id, job.target,
            );
            let detail = crate::redact::redact(&detail, &self.secret_refs());
            let _ = self
                .record_stage(
                    &identity,
                    job,
                    EventResult::Failed {
                        reason: detail.clone(),
                    },
                    LogLevel::Error,
                    &detail,
                )
                .await;
            return Ok(DispatchOutcome::Failed(detail));
        }
        let outcome = self.provider.dispatch(job).await?;
        let (result, level, summary) = match &outcome {
            DispatchOutcome::Ready(resource) => (
                EventResult::Succeeded,
                LogLevel::Info,
                format!("job {} on '{}' ready (resource {resource})", job.id, job.target),
            ),
            DispatchOutcome::Degraded(resource) => (
                EventResult::Succeeded,
                LogLevel::Warn,
                format!(
                    "job {} on '{}' degraded (resource {resource})",
                    job.id, job.target
                ),
            ),
            DispatchOutcome::InProgress => (
                EventResult::Accepted,
                LogLevel::Info,
                format!(
                    "job {} on '{}' accepted but not ready; promotion waits for platform observation",
                    job.id, job.target
                ),
            ),
            DispatchOutcome::Failed(reason) => (
                EventResult::Failed {
                    reason: reason.clone(),
                },
                LogLevel::Error,
                format!("job {} on '{}' failed: {reason}", job.id, job.target),
            ),
        };
        let detail = format!(
            "{summary} (application {}, trace {})",
            identity.application_id, identity.trace_id,
        );
        // Redact here, not only at the store boundary: the outcome string is
        // also returned to the worker, which persists it as the job error.
        let detail = crate::redact::redact(&detail, &self.secret_refs());
        let _ = self
            .record_stage(&identity, job, result, level, &detail)
            .await;
        Ok(outcome)
    }
}

/// No-op dispatcher for a daemon without a container runtime.
///
/// It performs no provider, container, or preview call; every dispatch records
/// the environment blocker (with recovery) to the platform event stream and
/// the separated runtime log, then reports a retryable failure. Jobs can never
/// read as executed through this dispatcher.
#[derive(Clone)]
pub struct UnavailableDispatcher {
    reason: String,
    events: PgEventStore,
    logs: PgLogStore,
    secrets: Vec<String>,
}

impl UnavailableDispatcher {
    pub fn new(pool: PgPool, reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            events: PgEventStore::new(pool.clone()),
            logs: PgLogStore::new(pool),
            secrets: Vec::new(),
        }
    }

    pub fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Why this dispatcher refuses execution.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }
}

#[async_trait]
impl JobDispatcher for UnavailableDispatcher {
    async fn dispatch(&self, job: &Job) -> Result<DispatchOutcome> {
        let detail = format!(
            "job {} on '{}' blocked: {} ; recovery: {BLOCKER_RECOVERY}",
            job.id, job.target, self.reason,
        );
        // Redact before returning: the worker persists this string as the job
        // error, and the target may carry a credential-bearing value.
        let detail = crate::redact::redact(&detail, &self.secret_refs());
        let correlation = identity_for_job(job)
            .map(|identity| identity.correlation())
            .unwrap_or_else(|_| Correlation::new(format!("job:{}", job.id)));
        let mut draft = EventDraft::new(
            Utc::now(),
            EventActor::platform(DISPATCHER_ACTOR),
            correlation.clone(),
            EventAction::Reconciled,
            EventResult::Failed {
                reason: detail.clone(),
            },
        );
        draft.resource = Some(EventResource::new("job", job.id.to_string()));
        draft.after = Some(detail.clone());
        let _ = self.events.append(&draft, &self.secret_refs()).await;
        let _ = self
            .logs
            .append(
                LogSource::Runtime,
                LogLevel::Error,
                &correlation,
                &detail,
                &self.secret_refs(),
                Utc::now(),
            )
            .await;
        Ok(DispatchOutcome::Failed(detail))
    }
}

/// What [`select_dispatcher`] chose at startup.
pub struct SelectedDispatcher {
    pub dispatcher: Arc<dyn JobDispatcher>,
    pub availability: RuntimeAvailability,
    /// True when jobs will report an environment blocker instead of executing.
    pub blocked: bool,
}

impl std::fmt::Debug for SelectedDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedDispatcher")
            .field("availability", &self.availability)
            .field("blocked", &self.blocked)
            .finish_non_exhaustive()
    }
}

/// Selects the daemon dispatcher from process configuration.
///
/// - `disabled` never probes: the daemon starts blocked with the recovery action.
/// - `auto` probes `LABRYS_DOCKER_BIN`: reachable runs the
///   [`ExecutableDispatcher`], unreachable starts blocked.
/// - `docker` probes and fails startup when the runtime is unreachable,
///   because explicitly requesting a runtime that is not there is a
///   misconfiguration, not a state to reconcile through.
pub async fn select_dispatcher(pool: &PgPool, config: &Config) -> Result<SelectedDispatcher> {
    if config.runtime_mode == RuntimeMode::Disabled {
        let reason =
            "container runtime explicitly disabled via LABRYS_RUNTIME_MODE=disabled".to_string();
        return Ok(SelectedDispatcher {
            dispatcher: Arc::new(UnavailableDispatcher::new(pool.clone(), reason.clone())),
            availability: RuntimeAvailability::Unavailable { reason },
            blocked: true,
        });
    }
    let executor = DockerExecutor::new(config.docker_bin.clone());
    let availability = executor.check_available().await;
    match &availability {
        RuntimeAvailability::Available { .. } => Ok(SelectedDispatcher {
            dispatcher: Arc::new(
                ExecutableDispatcher::build(pool.clone(), config)?
                    .with_availability(availability.clone()),
            ),
            availability,
            blocked: false,
        }),
        RuntimeAvailability::Unavailable { reason } => {
            if config.runtime_mode == RuntimeMode::Docker {
                return Err(ControlPlaneError::Config(format!(
                    "LABRYS_RUNTIME_MODE=docker but the container runtime is unreachable: \
                     {reason}; recovery: {BLOCKER_RECOVERY}"
                )));
            }
            Ok(SelectedDispatcher {
                dispatcher: Arc::new(UnavailableDispatcher::new(pool.clone(), reason.clone())),
                availability,
                blocked: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labrys_core::{IdempotencyKey, JobAction, JobId, JobStatus, RetryPolicy};

    fn job_with_target(target: &str) -> Job {
        let now = Utc::now();
        Job {
            id: JobId::new(),
            target: target.to_string(),
            action: JobAction::Provision,
            idempotency_key: IdempotencyKey::new(target, JobAction::Provision, 1),
            status: JobStatus::Queued,
            attempts: 0,
            policy: RetryPolicy::default(),
            next_attempt_at: now,
            last_error: None,
            enqueued_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn identity_derives_application_and_trace_from_job() {
        let job = job_with_target("some-target:deploy:staging");
        let identity = identity_for_job(&job).unwrap();
        assert_eq!(identity.application_id, "some-target");
        assert_eq!(identity.environment_id.as_deref(), Some("staging"));
        assert_eq!(identity.trace_id, format!("job:{}", job.id));
    }

    #[test]
    fn identity_keeps_plain_targets_whole() {
        let job = job_with_target("plain-target");
        let identity = identity_for_job(&job).unwrap();
        assert_eq!(identity.application_id, "plain-target");
        assert!(identity.environment_id.is_none());
    }

    #[test]
    fn empty_target_refuses_identity_with_recovery() {
        let job = job_with_target("   ");
        let err = identity_for_job(&job).unwrap_err();
        assert!(err.to_string().contains("empty target"), "{err}");
    }

    #[tokio::test]
    async fn missing_runtime_binary_reports_unavailable_without_simulation() {
        let executor = DockerExecutor::new(std::path::PathBuf::from(
            "/nonexistent-labrys-docker-binary",
        ));
        let availability = executor.check_available().await;
        assert!(!availability.is_available());
        assert!(availability.require().is_err());
    }
}
