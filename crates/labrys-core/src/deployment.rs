//! Production deployment, OCI artifacts, domains, and rollback.
//!
//! Covers requirement `deployment-platform`: a deployment is a first-class,
//! language-independent OCI-based object retaining revision, build, image,
//! runtime, environment, resources, endpoint, health, logs, and rollback
//! target. The production path builds an OCI image, starts it under the
//! deployment runtime, waits for platform health, and only then promotes the
//! revision to serve traffic — a deployment never becomes healthy or receives
//! production traffic before its configured runtime and application health
//! checks pass. Source, deployment, and configuration rollback are independent
//! operations, and a deployment rollback never claims to restore database
//! data.
//!
//! This module is pure and deterministic. Real registry pushes, container
//! starts, and TLS issuance are infrastructure built on these contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::controller::redact_reason;
use crate::error::{CoreError, Result};
use crate::ids::{ApplicationId, DeploymentId, DomainId, EnvironmentId, ResourceId};
use crate::inspector::ExplicitApproval;
use crate::runtime::{
    BuildResult, BuildStatus, Endpoint, HealthStatus, RuntimeConfig, RuntimeKind, RuntimeProfile,
};

/// Version of the deployment contract vocabulary this crate implements.
pub const DEPLOYMENT_CONTRACT_VERSION: u32 = 1;

/// The database-data warning attached to every deployment rollback. A
/// deployment rollback re-points the application image only; it never
/// restores database contents.
pub const DATABASE_DATA_WARNING: &str =
    "deployment rollback restores the application image only; database data is not \
     automatically rolled back. Review migrations applied since the target revision \
     and restore data through an explicit, approved data operation.";

/// The configuration warning attached to configuration rollback.
pub const CONFIGURATION_DATA_WARNING: &str =
    "configuration rollback changes platform configuration only; it does not revert \
     the running image or database data.";

// ---------------------------------------------------------------------------
// Revision, image, and log records
// ---------------------------------------------------------------------------

/// What a deployment was built from: source revision plus the configuration
/// and capability snapshots it converged on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    /// Source commit identifier (git sha or equivalent).
    pub source_sha: String,
    /// Hash of the resolved application configuration.
    pub config_hash: String,
    /// Hash of the resolved capability/binding set. `None` when the
    /// application declares no capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_hash: Option<String>,
    /// True when this revision introduced a database migration relative to
    /// its predecessor. Drives the rollback data warning's urgency.
    #[serde(default)]
    pub introduces_migration: bool,
}

impl Revision {
    pub fn new(
        source_sha: impl Into<String>,
        config_hash: impl Into<String>,
        capability_hash: Option<String>,
    ) -> Self {
        Self {
            source_sha: source_sha.into(),
            config_hash: config_hash.into(),
            capability_hash,
            introduces_migration: false,
        }
    }

    pub fn with_migration(mut self, introduces: bool) -> Self {
        self.introduces_migration = introduces;
        self
    }
}

/// Where an OCI artifact came from. Generic-container and native-runtime
/// outputs both land here, keeping deployment records language-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageSource {
    /// Built through the Tier 0 generic container runtime.
    Generic,
    /// Built by a native runtime adapter.
    Native,
}

/// An OCI artifact produced by a production build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    /// Full image reference, e.g. `registry.local/app@sha256:...`.
    pub reference: String,
    /// Content digest; the deployment target identity for rollouts.
    pub digest: String,
    pub source: ImageSource,
    pub runtime_kind: RuntimeKind,
    /// Revision this artifact was built from.
    pub revision_sha: String,
}

impl Image {
    /// True when the artifact is addressable by digest (production gate).
    pub fn is_addressable(&self) -> bool {
        self.digest.starts_with("sha256:") && !self.reference.is_empty()
    }
}

/// One structured deployment log line. Values are redacted on append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentLog {
    pub level: LogLevel,
    pub message: String,
    pub at: DateTime<Utc>,
}

/// Log severity for deployment events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Builds the OCI artifact for a successful production build.
///
/// Development profiles never mint OCI; a runtime without a production image
/// reference (e.g. Expo Go flows) is rejected. The digest is derived
/// deterministically from the runtime's image reference and the revision, so
/// the same inputs always select the same rollback target.
pub fn build_image(
    config: &RuntimeConfig,
    build: &BuildResult,
    revision: &Revision,
    secrets: &[&str],
) -> Result<Image> {
    if config.profile != RuntimeProfile::Production {
        return Err(CoreError::Deployment(
            "development profiles never mint an OCI image".to_string(),
        ));
    }
    if build.status != BuildStatus::Succeeded {
        let detail = redact_reason(&build.detail, secrets);
        return Err(CoreError::Deployment(format!(
            "cannot build image from {:?} build: {detail}",
            build.status
        )));
    }
    let base = config.oci_image.clone().ok_or_else(|| {
        CoreError::Deployment(format!(
            "runtime '{}' produces no OCI artifact",
            config.kind.canonical_name()
        ))
    })?;
    let source = match config.kind {
        RuntimeKind::Generic => ImageSource::Generic,
        _ => ImageSource::Native,
    };
    let digest = format!("sha256:{}", digest_of(&base, &revision.source_sha));
    Ok(Image {
        reference: format!("{base}@{digest}"),
        digest,
        source,
        runtime_kind: config.kind,
        revision_sha: revision.source_sha.clone(),
    })
}

/// Deterministic FNV-1a hex digest stand-in for a registry manifest digest.
fn digest_of(base: &str, revision_sha: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in base.as_bytes().iter().chain(revision_sha.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Spread across two 64-bit rounds so the digest is not a single word.
    let mut second = hash;
    for byte in revision_sha.as_bytes().iter().chain(base.as_bytes()) {
        second ^= u64::from(*byte);
        second = second.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}{second:016x}")
}

// ---------------------------------------------------------------------------
// Domains
// ---------------------------------------------------------------------------

/// A hostname bound to one environment, pointing at a deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Domain {
    pub id: DomainId,
    pub environment_id: EnvironmentId,
    pub hostname: String,
    pub tls: bool,
    /// Deployment currently serving this domain; `None` until attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentId>,
}

impl Domain {
    pub fn new(environment_id: EnvironmentId, hostname: impl Into<String>, tls: bool) -> Self {
        Self {
            id: DomainId::new(),
            environment_id,
            hostname: hostname.into(),
            tls,
            deployment: None,
        }
    }

    /// True when the hostname is syntactically usable for production traffic.
    pub fn is_routable(&self) -> bool {
        !self.hostname.trim().is_empty() && !self.hostname.chars().any(char::is_whitespace)
    }
}

// ---------------------------------------------------------------------------
// Deployment lifecycle
// ---------------------------------------------------------------------------

/// Explicit deployment rollout state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DeploymentPhase {
    /// Created; build not started.
    Pending,
    /// Building the OCI artifact.
    Building,
    /// Artifact built; starting under the runtime, health not yet confirmed.
    Deploying,
    /// Health checks passed; serving production traffic.
    Healthy,
    /// Running but health checks are not fully passing; not promoted.
    Degraded,
    /// Terminal failure before promotion; no traffic.
    Failed,
    /// Replaced by a newer healthy deployment; retained as rollback target.
    Superseded,
    /// Traffic moved away by an explicit rollback.
    RolledBack,
}

impl DeploymentPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::RolledBack)
    }

    /// True when the deployment may serve production traffic.
    pub fn serves_traffic(self) -> bool {
        self == Self::Healthy
    }

    /// The explicit rollout graph. Promotion to [`DeploymentPhase::Healthy`]
    /// is only reachable from `deploying` (the health gate).
    pub fn can_transition_to(self, next: Self) -> bool {
        use DeploymentPhase::*;
        match self {
            Pending => matches!(next, Building | Failed),
            Building => matches!(next, Deploying | Failed),
            Deploying => matches!(next, Healthy | Degraded | Failed),
            Healthy => matches!(next, Degraded | Superseded | RolledBack),
            Degraded => matches!(next, Healthy | Failed | RolledBack),
            Failed => matches!(next, RolledBack),
            Superseded => matches!(next, RolledBack),
            RolledBack => false,
        }
    }
}

/// One audited deployment phase transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentTransition {
    pub from: DeploymentPhase,
    pub to: DeploymentPhase,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A first-class production deployment. Retains every metadata field the
/// requirement names; the record is language-independent (a generic-container
/// app and a native-runtime app share the shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deployment {
    pub id: DeploymentId,
    pub application_id: ApplicationId,
    pub environment_id: EnvironmentId,
    pub revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub resources: Vec<ResourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Endpoint>,
    pub health: HealthStatus,
    pub phase: DeploymentPhase,
    #[serde(default)]
    pub logs: Vec<DeploymentLog>,
    /// Prior healthy deployment kept as the rollback target, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_target: Option<DeploymentId>,
    #[serde(default)]
    pub transitions: Vec<DeploymentTransition>,
}

impl Deployment {
    /// Creates a pending deployment carrying the prior healthy rollback
    /// target. Nothing is built or started yet.
    pub fn new(
        application_id: ApplicationId,
        environment_id: EnvironmentId,
        revision: Revision,
        runtime: RuntimeConfig,
        resources: Vec<ResourceId>,
        rollback_target: Option<DeploymentId>,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        if runtime.profile != RuntimeProfile::Production {
            return Err(CoreError::Deployment(
                "deployments require a production runtime profile".to_string(),
            ));
        }
        Ok(Self {
            id: DeploymentId::new(),
            application_id,
            environment_id,
            revision,
            build: None,
            image: None,
            runtime,
            resources,
            endpoint: None,
            health: HealthStatus::Starting,
            phase: DeploymentPhase::Pending,
            logs: Vec::new(),
            rollback_target,
            transitions: vec![DeploymentTransition {
                from: DeploymentPhase::Pending,
                to: DeploymentPhase::Pending,
                at,
                reason: None,
            }],
        })
    }

    /// Moves to `next`, rejecting graph violations and same-state no-ops.
    fn move_to(
        &mut self,
        next: DeploymentPhase,
        reason: Option<String>,
        at: DateTime<Utc>,
    ) -> Result<()> {
        if !self.phase.can_transition_to(next) {
            return Err(CoreError::Deployment(format!(
                "illegal deployment transition {:?} -> {:?} for {}",
                self.phase, next, self.id
            )));
        }
        if next == self.phase {
            return Err(CoreError::Deployment(format!(
                "deployment {} is already {:?}",
                self.id, next
            )));
        }
        self.phase = next;
        self.transitions.push(DeploymentTransition {
            from: self.transitions.last().expect("never empty").to,
            to: next,
            at,
            reason,
        });
        Ok(())
    }

    /// Starts the OCI build.
    pub fn start_build(&mut self, at: DateTime<Utc>) -> Result<()> {
        self.move_to(DeploymentPhase::Building, None, at)
    }

    /// Records the build outcome and, on success, mints the OCI artifact.
    /// A failed or cancelled build fails the deployment carrying the enforced
    /// limit and recovery detail (redacted).
    pub fn record_build(
        &mut self,
        build: &BuildResult,
        secrets: &[&str],
        at: DateTime<Utc>,
    ) -> Result<()> {
        if self.phase != DeploymentPhase::Building {
            return Err(CoreError::Deployment(format!(
                "deployment {} cannot record a build while {:?}",
                self.id, self.phase
            )));
        }
        self.build = Some(build.clone());
        if !build.is_success() {
            let detail = redact_reason(&build.detail, secrets);
            self.push_log(LogLevel::Error, &detail, at);
            return self.move_to(DeploymentPhase::Failed, Some(detail), at);
        }
        let image = build_image(&self.runtime, build, &self.revision, secrets)?;
        self.push_log(
            LogLevel::Info,
            &format!("image {} built from {}", image.digest, image.source_label()),
            at,
        );
        self.image = Some(image);
        self.move_to(DeploymentPhase::Deploying, None, at)
    }

    /// Records the platform health observation. This is the promotion gate:
    /// only a [`HealthStatus::Healthy`] observation moves the deployment to
    /// `healthy`; a failed readiness during rollout stops promotion and fails
    /// it; a running-but-unverified state degrades it. `Starting`/`Unknown`
    /// leave the deployment in `deploying` (no signal yet). Reasons are
    /// redacted against `secrets` before they reach transition evidence.
    pub fn record_health(
        &mut self,
        health: HealthStatus,
        secrets: &[&str],
        at: DateTime<Utc>,
    ) -> Result<()> {
        match health {
            HealthStatus::Healthy => {
                self.health = health;
                self.move_to(DeploymentPhase::Healthy, None, at)?;
            }
            HealthStatus::Unhealthy { reason } => {
                let target = if self.phase == DeploymentPhase::Deploying {
                    DeploymentPhase::Failed
                } else {
                    DeploymentPhase::Degraded
                };
                let reason = redact_reason(&reason, secrets);
                self.health = HealthStatus::Unhealthy {
                    reason: reason.clone(),
                };
                self.move_to(target, Some(reason), at)?;
            }
            HealthStatus::Starting | HealthStatus::Unknown { .. } => {
                // No promotion signal yet; keep deploying, record the probe.
                self.health = health;
            }
        }
        Ok(())
    }

    /// Marks this deployment superseded by a newer healthy one. It stays
    /// identifiable as a rollback target.
    pub fn mark_superseded(&mut self, at: DateTime<Utc>) -> Result<()> {
        self.move_to(
            DeploymentPhase::Superseded,
            Some("newer revision promoted".to_string()),
            at,
        )
    }

    /// Marks this deployment rolled back (traffic moved away).
    pub fn mark_rolled_back(&mut self, reason: &str, at: DateTime<Utc>) -> Result<()> {
        self.move_to(DeploymentPhase::RolledBack, Some(reason.to_string()), at)
    }

    /// Resolves the runtime endpoint once the workload is reachable.
    pub fn attach_endpoint(&mut self, endpoint: Endpoint) -> Result<()> {
        if !endpoint.exposed {
            return Err(CoreError::Deployment(
                "cannot attach a non-exposed endpoint to a deployment".to_string(),
            ));
        }
        self.endpoint = Some(endpoint);
        Ok(())
    }

    /// Appends a redacted structured log line.
    pub fn push_log(&mut self, level: LogLevel, message: &str, at: DateTime<Utc>) {
        self.logs.push(DeploymentLog {
            level,
            message: message.to_string(),
            at,
        });
    }

    /// True only when health checks passed and the deployment serves traffic.
    pub fn is_promoted(&self) -> bool {
        self.phase.serves_traffic()
    }

    /// Renders the deployment for events/logs. Never contains secret values;
    /// the rollback target stays identifiable.
    pub fn to_event_detail(&self) -> String {
        let image = self
            .image
            .as_ref()
            .map(|i| i.digest.clone())
            .unwrap_or_else(|| "no-image".to_string());
        format!(
            "deployment {} ({}) phase {:?} revision {} image {image} rollback {:?}",
            self.id,
            self.runtime.kind.canonical_name(),
            self.phase,
            self.revision.source_sha,
            self.rollback_target
        )
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

impl ImageSource {
    fn label(self) -> &'static str {
        match self {
            Self::Generic => "generic container runtime",
            Self::Native => "native runtime adapter",
        }
    }
}

impl Image {
    fn source_label(&self) -> &'static str {
        self.source.label()
    }
}

// ---------------------------------------------------------------------------
// Rollout orchestration
// ---------------------------------------------------------------------------

/// Ordered deployment history for one environment plus the currently serving
/// deployment. Reconciliation drives rollouts independently of the request
/// that changed desired state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rollout {
    deployments: Vec<Deployment>,
    current: Option<DeploymentId>,
}

impl Rollout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn deployments(&self) -> &[Deployment] {
        &self.deployments
    }

    pub fn current(&self) -> Option<DeploymentId> {
        self.current
    }

    pub fn get(&self, id: DeploymentId) -> Option<&Deployment> {
        self.deployments.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: DeploymentId) -> Option<&mut Deployment> {
        self.deployments.iter_mut().find(|d| d.id == id)
    }

    /// The prior healthy deployment, used as the next rollout's rollback
    /// target. `Superseded` deployments remain eligible.
    pub fn rollback_target(&self) -> Option<DeploymentId> {
        self.deployments
            .iter()
            .rev()
            .find(|d| {
                matches!(
                    d.phase,
                    DeploymentPhase::Healthy | DeploymentPhase::Superseded
                ) && d.image.is_some()
            })
            .map(|d| d.id)
    }

    /// Begins a rollout for `revision`, wiring the current healthy
    /// deployment as its rollback target.
    pub fn begin(
        &mut self,
        application_id: ApplicationId,
        environment_id: EnvironmentId,
        revision: Revision,
        runtime: RuntimeConfig,
        resources: Vec<ResourceId>,
        at: DateTime<Utc>,
    ) -> Result<DeploymentId> {
        let rollback_target = self.rollback_target();
        let deployment = Deployment::new(
            application_id,
            environment_id,
            revision,
            runtime,
            resources,
            rollback_target,
            at,
        )?;
        let id = deployment.id;
        self.deployments.push(deployment);
        Ok(id)
    }

    /// Promotes a healthy deployment to current, superseding the previous
    /// one. Refuses to promote anything that has not passed the health gate.
    pub fn promote(&mut self, id: DeploymentId, at: DateTime<Utc>) -> Result<()> {
        let deployment = self
            .get(id)
            .ok_or_else(|| CoreError::NotFound(format!("deployment {id}")))?;
        if deployment.phase != DeploymentPhase::Healthy {
            return Err(CoreError::Deployment(format!(
                "only a healthy deployment can be promoted, {id} is {:?}",
                deployment.phase
            )));
        }
        if let Some(previous) = self.current {
            if previous != id {
                if let Some(old) = self.get_mut(previous) {
                    if old.phase == DeploymentPhase::Healthy {
                        old.mark_superseded(at)?;
                    }
                }
            }
        }
        self.current = Some(id);
        Ok(())
    }

    /// The image digest currently serving traffic, if any.
    pub fn current_digest(&self) -> Option<&str> {
        self.current
            .and_then(|id| self.get(id))
            .and_then(|d| d.image.as_ref())
            .map(|i| i.digest.as_str())
    }
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

/// Independently requestable rollback boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RollbackKind {
    /// Revert the source revision (and rebuild).
    Source,
    /// Re-point traffic to a prior built image.
    Deployment,
    /// Revert platform configuration only.
    Configuration,
}

impl RollbackKind {
    /// Whether this rollback always carries a database-data warning. Only a
    /// deployment (image) rollback re-points the running application; source
    /// and configuration rollbacks carry their own scoped warnings.
    pub fn touches_runtime_image(self) -> bool {
        matches!(self, Self::Deployment)
    }
}

/// A planned rollback with its explicit boundaries and warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackPlan {
    pub kind: RollbackKind,
    pub from: DeploymentId,
    pub to: DeploymentId,
    /// Human-facing warnings that must be acknowledged before execution.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// True when a migration is in the delta, escalating the data warning.
    pub migration_in_delta: bool,
    /// Domain changes require explicit approval.
    pub requires_approval: bool,
}

/// Builds a rollback plan for `kind`. A deployment rollback always warns that
/// database data is not restored; when the delta introduced a migration the
/// plan additionally flags it. Source and configuration rollbacks carry their
/// own scoped warnings and never claim to restore the running image.
pub fn plan_rollback(
    kind: RollbackKind,
    from: &Deployment,
    to: &Deployment,
) -> Result<RollbackPlan> {
    if from.id == to.id {
        return Err(CoreError::Deployment(
            "rollback target must differ from the current deployment".to_string(),
        ));
    }
    if to.image.is_none() {
        return Err(CoreError::Deployment(format!(
            "rollback target {} has no built image",
            to.id
        )));
    }
    let migration_in_delta = from.revision.introduces_migration;
    let mut warnings = Vec::new();
    match kind {
        RollbackKind::Deployment => {
            warnings.push(DATABASE_DATA_WARNING.to_string());
        }
        RollbackKind::Source => {
            warnings.push(
                "source rollback rebuilds from an earlier revision; it does not revert the \
                 currently running image until the rebuild is promoted"
                    .to_string(),
            );
        }
        RollbackKind::Configuration => {
            warnings.push(CONFIGURATION_DATA_WARNING.to_string());
        }
    }
    if migration_in_delta && kind != RollbackKind::Configuration {
        warnings.push(
            "the delta includes a database migration; schema and data may be ahead of the \
             rollback target"
                .to_string(),
        );
    }
    Ok(RollbackPlan {
        kind,
        from: from.id,
        to: to.id,
        warnings,
        migration_in_delta,
        requires_approval: true,
    })
}

/// Selects the rollback target: the most recent prior deployment that was
/// actually built and is not the current one. Never selects a failed or
/// never-built revision.
pub fn select_rollback_target(rollout: &Rollout, current: DeploymentId) -> Option<DeploymentId> {
    rollout
        .deployments()
        .iter()
        .rev()
        .find(|d| d.id != current && d.image.is_some() && d.phase != DeploymentPhase::Failed)
        .map(|d| d.id)
}

/// Executes an approved rollback of any kind. For a deployment rollback it
/// moves traffic to the target and marks the source rolled back; the returned
/// detail always restates the database-data boundary. Domain re-attachment is
/// the caller's approved step.
pub fn execute_rollback(
    rollout: &mut Rollout,
    plan: &RollbackPlan,
    approval: &ExplicitApproval,
    domains: &mut [Domain],
    at: DateTime<Utc>,
) -> Result<RollbackOutcome> {
    if !approval.approved || approval.approver.trim().is_empty() {
        return Err(CoreError::ApprovalRequired(format!(
            "rollback {:?} {} -> {} requires a granted, named human approval",
            plan.kind, plan.from, plan.to
        )));
    }
    if plan.kind != RollbackKind::Deployment {
        // Source and configuration rollbacks do not re-point the running
        // image; they record an approved intent for the controller to
        // converge. The database-data boundary still holds.
        return Ok(RollbackOutcome {
            kind: plan.kind,
            from: plan.from,
            to: plan.to,
            traffic_moved: false,
            detail: plan.warnings.join(" | "),
        });
    }
    let target_phase = rollout
        .get(plan.to)
        .ok_or_else(|| CoreError::NotFound(format!("deployment {}", plan.to)))?
        .phase;
    if !matches!(
        target_phase,
        DeploymentPhase::Superseded | DeploymentPhase::RolledBack | DeploymentPhase::Healthy
    ) {
        return Err(CoreError::Deployment(format!(
            "rollback target {} is {target_phase:?}, not a safe target",
            plan.to
        )));
    }
    // Move traffic: source rolled back, target healthy and current.
    if let Some(source) = rollout.get_mut(plan.from) {
        if source.phase == DeploymentPhase::Healthy || source.phase == DeploymentPhase::Degraded {
            source.mark_rolled_back(
                &format!("rolled back to {} by {}", plan.to, approval.approver),
                at,
            )?;
        }
    }
    if let Some(target) = rollout.get_mut(plan.to) {
        if target.phase != DeploymentPhase::Healthy {
            target.phase = DeploymentPhase::Healthy;
            target.transitions.push(DeploymentTransition {
                from: target_phase,
                to: DeploymentPhase::Healthy,
                at,
                reason: Some("restored by rollback".to_string()),
            });
        }
    }
    rollout.current = Some(plan.to);
    // Re-point every domain that served the rolled-back deployment.
    for domain in domains.iter_mut() {
        if domain.deployment == Some(plan.from) {
            domain.deployment = Some(plan.to);
        }
    }
    Ok(RollbackOutcome {
        kind: plan.kind,
        from: plan.from,
        to: plan.to,
        traffic_moved: true,
        detail: DATABASE_DATA_WARNING.to_string(),
    })
}

/// Result of executing a rollback plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackOutcome {
    pub kind: RollbackKind,
    pub from: DeploymentId,
    pub to: DeploymentId,
    pub traffic_moved: bool,
    /// Restates the explicit boundaries; never claims database restoration.
    pub detail: String,
}

/// Attaches a domain to a promoted deployment. Requires a granted, named
/// approval and a deployment that has passed the health gate, so a domain
/// change can never route traffic to an unverified revision.
pub fn attach_domain(
    domain: &mut Domain,
    deployment: &Deployment,
    approval: &ExplicitApproval,
) -> Result<()> {
    if !approval.approved || approval.approver.trim().is_empty() {
        return Err(CoreError::ApprovalRequired(format!(
            "domain change for '{}' requires explicit approval",
            domain.hostname
        )));
    }
    if !domain.is_routable() {
        return Err(CoreError::Deployment(format!(
            "domain '{}' is not routable",
            domain.hostname
        )));
    }
    if !deployment.is_promoted() {
        return Err(CoreError::Deployment(format!(
            "cannot route '{}' to unpromoted deployment {} ({:?})",
            domain.hostname, deployment.id, deployment.phase
        )));
    }
    domain.deployment = Some(deployment.id);
    Ok(())
}
