//! Provider, registry, and domain delivery contracts.
//!
//! Covers `provider-registry-and-domain-adapters`: capability provisioning,
//! OCI registry delivery, and DNS/TLS/traffic attachment execute through
//! explicit provider adapters behind controller jobs. The core stays I/O-free:
//! this module plans scoped operations, gates destructive actions on named
//! human approval, redacts credential material from every diagnostic, and
//! records digest/certificate evidence. Real network, registry, DNS, and TLS
//! I/O belongs to infrastructure built on these contracts.
//!
//! Boundaries:
//! - A provider acceptance never flips readiness: the resource stays
//!   `provisioning` until a later platform observation marks it ready. An
//!   agent completion claim is not an observation and never promotes.
//! - Destructive operations (delete, replace, migrate, adopt) never reach a
//!   provider without a granted, named approval matching the target scope.
//! - Registry delivery accepts only verified production OCI artifacts and
//!   refuses promotion on digest mismatch, with recovery guidance.
//! - Traffic attaches only to a promoted healthy deployment behind propagated
//!   DNS and issued TLS; a certificate failure reports the failure, never
//!   healthy, and attaches nothing.

use serde::{Deserialize, Serialize};

use crate::controller::{redact_reason, ResourcePhase};
use crate::deployment::{Deployment, Domain};
use crate::error::{CoreError, Result};
use crate::ids::{ApplicationId, DeploymentId, DomainId, EnvironmentId, ResourceId};
use crate::inspector::ExplicitApproval;

/// Version of the provider/delivery contract vocabulary.
pub const PROVIDER_CONTRACT_VERSION: u32 = 1;

/// Recovery guidance attached when a provider failure is sanitized.
pub const PROVIDER_FAILURE_RECOVERY: &str =
    "retry with backoff; inspect redacted provider diagnostics and re-run the platform readiness probe before promoting";
/// Recovery guidance attached to a registry digest mismatch.
pub const DIGEST_MISMATCH_RECOVERY: &str =
    "registry digest differs from the recorded build artifact; do not promote. Re-push the verified production image and re-verify the digest before deploying";
/// Recovery guidance attached to a certificate failure.
pub const CERTIFICATE_FAILURE_RECOVERY: &str =
    "TLS issuance failed; traffic was not attached. Fix DNS/issuer configuration and re-issue the certificate before attaching traffic";

/// Which external family an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderKind {
    Postgres,
    Auth,
    FileStorage,
    ObjectStorage,
    Generic,
}

impl ProviderKind {
    /// Capability keys this kind can supply.
    pub fn supplies(self, capability: &str) -> bool {
        match self {
            Self::Postgres => matches!(capability, "database.postgres" | "database"),
            Self::Auth => matches!(capability, "auth" | "auth.session"),
            Self::FileStorage => matches!(capability, "storage.file" | "storage"),
            Self::ObjectStorage => matches!(capability, "storage.object" | "storage"),
            Self::Generic => true,
        }
    }
}

/// What the operation asks the provider to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderAction {
    Provision,
    ReadinessCheck,
    Replace,
    Migrate,
    Adopt,
    Delete,
}

impl ProviderAction {
    /// Destructive actions never reach a provider without approval.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            Self::Delete | Self::Replace | Self::Migrate | Self::Adopt
        )
    }
}

/// One scoped provider operation. Every field the adapter needs to scope,
/// retry idempotently, attribute, and audit the call travels here; credential
/// values never travel here, only secret reference ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOperation {
    pub application_id: ApplicationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<EnvironmentId>,
    pub resource_id: ResourceId,
    pub capability: String,
    pub provider_key: String,
    pub kind: ProviderKind,
    pub action: ProviderAction,
    /// Idempotency key scoping retries and duplicate requests to one effect.
    pub idempotency_key: String,
    /// Trace tying the operation to its events, logs, and evidence.
    pub trace_id: String,
    /// Secret reference ids the adapter may inject at operation time.
    #[serde(default)]
    pub secret_refs: Vec<String>,
    /// Retry attempt number, 1-based.
    #[serde(default = "default_attempt")]
    pub attempt: u32,
}

fn default_attempt() -> u32 {
    1
}

impl ProviderOperation {
    /// Plans a scoped operation. Rejects empty capability/provider/trace ids so
    /// an unscoped call can never be constructed.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        application_id: ApplicationId,
        environment_id: Option<EnvironmentId>,
        resource_id: ResourceId,
        capability: impl Into<String>,
        provider_key: impl Into<String>,
        kind: ProviderKind,
        action: ProviderAction,
        trace_id: impl Into<String>,
    ) -> Result<Self> {
        let capability = capability.into();
        let provider_key = provider_key.into();
        let trace_id = trace_id.into();
        if capability.trim().is_empty() {
            return Err(CoreError::CapabilityBinding(
                "provider operation requires a capability key".to_string(),
            ));
        }
        if provider_key.trim().is_empty() {
            return Err(CoreError::CapabilityBinding(
                "provider operation requires a provider key".to_string(),
            ));
        }
        if trace_id.trim().is_empty() {
            return Err(CoreError::CapabilityBinding(
                "provider operation requires a trace id".to_string(),
            ));
        }
        if !kind.supplies(&capability) {
            return Err(CoreError::CapabilityBinding(format!(
                "provider kind {kind:?} does not supply capability '{capability}'"
            )));
        }
        let idempotency_key = format!(
            "{}:{}:{}:{}",
            resource_id,
            provider_key,
            action_label(action),
            trace_id
        );
        Ok(Self {
            application_id,
            environment_id,
            resource_id,
            capability,
            provider_key,
            kind,
            action,
            idempotency_key,
            trace_id,
            secret_refs: Vec::new(),
            attempt: 1,
        })
    }

    pub fn with_secret_refs(mut self, refs: Vec<String>) -> Self {
        self.secret_refs = refs;
        self
    }

    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt.max(1);
        self
    }

    /// Enforces the destructive-action gate before any provider call. Returns
    /// `Ok(())` only for non-destructive actions or a granted, named approval
    /// whose summary names the target resource scope.
    pub fn require_approval(&self, approval: Option<&ExplicitApproval>) -> Result<()> {
        if !self.action.is_destructive() {
            return Ok(());
        }
        let approval = approval.ok_or_else(|| {
            CoreError::ApprovalRequired(format!(
                "provider {:?} on resource {} requires a granted, named human approval",
                self.action, self.resource_id
            ))
        })?;
        if !approval.approved || approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(format!(
                "provider {:?} on resource {} was denied or unnamed; no provider call occurred",
                self.action, self.resource_id
            )));
        }
        if !approval
            .proposal_summary
            .contains(&self.resource_id.to_string())
            && !approval.proposal_summary.contains(&self.capability)
        {
            return Err(CoreError::ApprovalRequired(format!(
                "approval scope '{}' does not match resource {} capability '{}'; no provider call occurred",
                approval.proposal_summary, self.resource_id, self.capability
            )));
        }
        Ok(())
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

/// What a provider adapter observed. `Accepted` keeps the resource in
/// `provisioning`; only [`ProviderObservation::Ready`] observed through the
/// platform probe path may later mark it ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderObservation {
    Accepted { detail: String },
    Ready { detail: String },
    Degraded { reason: String },
    Failed { reason: String, recovery: String },
}

impl ProviderObservation {
    pub fn accepted(detail: impl Into<String>) -> Self {
        Self::Accepted {
            detail: detail.into(),
        }
    }

    /// Sanitizes a provider failure: redacts credential material and attaches
    /// recovery guidance. The sanitized reason is safe for events and logs.
    pub fn failed(reason: &str, secrets: &[&str]) -> Self {
        let (reason, recovery) = sanitize_failure(reason, secrets);
        Self::Failed { reason, recovery }
    }

    /// The resource phase this observation converges toward. `Accepted` maps
    /// to `provisioning` — never directly to ready.
    pub fn target_phase(self) -> ResourcePhase {
        match self {
            Self::Accepted { .. } => ResourcePhase::Provisioning,
            Self::Ready { .. } => ResourcePhase::Ready,
            Self::Degraded { .. } => ResourcePhase::Degraded,
            Self::Failed { .. } => ResourcePhase::Failed,
        }
    }

    /// Renders the observation for events/logs. Reasons are stored redacted.
    pub fn to_event_detail(&self) -> String {
        match self {
            Self::Accepted { detail } => format!("provider accepted: {detail}"),
            Self::Ready { detail } => format!("provider ready: {detail}"),
            Self::Degraded { reason } => format!("provider degraded: {reason}"),
            Self::Failed { reason, recovery } => {
                format!("provider failed: {reason} | recovery: {recovery}")
            }
        }
    }
}

/// Redacts credential material from a provider failure and attaches recovery
/// guidance. Never returns secret values.
pub fn sanitize_failure(reason: &str, secrets: &[&str]) -> (String, String) {
    (
        redact_reason(reason, secrets),
        PROVIDER_FAILURE_RECOVERY.to_string(),
    )
}

/// Records that an agent completion claim was seen and ignored: agent output
/// is never a readiness observation, so the resource phase does not move.
pub fn note_agent_claim_ignored(claims_ignored: &mut u32) {
    *claims_ignored = claims_ignored.saturating_add(1);
}

// ---------------------------------------------------------------------------
// Registry delivery
// ---------------------------------------------------------------------------

/// An OCI artifact candidate for registry delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryArtifact {
    pub reference: String,
    pub digest: String,
    pub revision_sha: String,
    /// True only when produced by a verified production build.
    pub verified_production_build: bool,
}

impl RegistryArtifact {
    pub fn new(
        reference: impl Into<String>,
        digest: impl Into<String>,
        revision_sha: impl Into<String>,
        verified_production_build: bool,
    ) -> Self {
        Self {
            reference: reference.into(),
            digest: digest.into(),
            revision_sha: revision_sha.into(),
            verified_production_build,
        }
    }

    pub fn is_digest_addressable(&self) -> bool {
        self.digest.starts_with("sha256:") && !self.reference.is_empty()
    }
}

/// Gates a registry push: only verified production, digest-addressable
/// artifacts may be pushed.
pub fn gate_registry_push(artifact: &RegistryArtifact) -> Result<()> {
    if !artifact.verified_production_build {
        return Err(CoreError::Deployment(
            "registry push requires a verified production build; development or unverified artifacts are refused"
                .to_string(),
        ));
    }
    if !artifact.is_digest_addressable() {
        return Err(CoreError::Deployment(format!(
            "registry push refused for non-digest-addressable artifact '{}'",
            artifact.reference
        )));
    }
    Ok(())
}

/// Verifies the registry-reported digest against the recorded build digest.
/// On mismatch the deployment must remain unpromoted; the returned error
/// carries the mismatch evidence plus recovery guidance.
pub fn verify_registry_digest(expected: &str, reported: &str) -> Result<()> {
    if expected == reported {
        return Ok(());
    }
    Err(CoreError::Deployment(format!(
        "registry digest mismatch: recorded {expected} but registry reported {reported}. {DIGEST_MISMATCH_RECOVERY}"
    )))
}

// ---------------------------------------------------------------------------
// Domain delivery: DNS, TLS, traffic
// ---------------------------------------------------------------------------

/// Observable DNS state for a hostname.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DnsState {
    Pending,
    Propagated,
    Failed { reason: String },
}

/// Observable certificate state for a hostname.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CertificateState {
    Pending,
    Issued,
    Renewing,
    Failed { reason: String },
}

impl CertificateState {
    pub fn failed(reason: &str, secrets: &[&str]) -> Self {
        Self::Failed {
            reason: redact_reason(reason, secrets),
        }
    }

    pub fn is_issued(&self) -> bool {
        matches!(self, Self::Issued | Self::Renewing)
    }
}

/// Separately observable domain delivery state: DNS, TLS, and traffic move
/// independently so a certificate failure can never read as healthy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainDelivery {
    pub domain_id: DomainId,
    pub deployment_id: Option<DeploymentId>,
    pub dns: DnsState,
    pub tls: CertificateState,
    pub traffic_attached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl DomainDelivery {
    pub fn new(domain_id: DomainId) -> Self {
        Self {
            domain_id,
            deployment_id: None,
            dns: DnsState::Pending,
            tls: CertificateState::Pending,
            traffic_attached: false,
            detail: None,
        }
    }

    /// True only when the domain serves traffic from a deployment.
    pub fn is_healthy(&self) -> bool {
        self.traffic_attached
            && self.deployment_id.is_some()
            && matches!(self.dns, DnsState::Propagated)
            && self.tls.is_issued()
    }

    /// Reports a certificate failure: traffic stays detached and the domain
    /// reports the failure rather than healthy.
    pub fn report_certificate_failure(&mut self, reason: &str, secrets: &[&str]) {
        self.tls = CertificateState::failed(reason, secrets);
        self.traffic_attached = false;
        if let CertificateState::Failed { reason } = &self.tls {
            self.detail = Some(format!(
                "certificate issuance failed: {reason}. {CERTIFICATE_FAILURE_RECOVERY}"
            ));
        }
    }

    pub fn to_event_detail(&self) -> String {
        format!(
            "domain {} dns {:?} tls {:?} traffic {} deployment {:?} {}",
            self.domain_id,
            self.dns,
            self.tls,
            self.traffic_attached,
            self.deployment_id,
            self.detail.as_deref().unwrap_or("")
        )
    }
}

/// Plans a traffic attachment. Separately gates approval, routability, DNS
/// propagation, TLS issuance, and the promoted-healthy deployment target. A
/// certificate failure returns an error naming the failure with recovery
/// guidance and attaches no traffic.
pub fn plan_traffic_attachment(
    delivery: &mut DomainDelivery,
    domain: &Domain,
    deployment: &Deployment,
    approval: &ExplicitApproval,
) -> Result<()> {
    if !approval.approved || approval.approver.trim().is_empty() {
        return Err(CoreError::ApprovalRequired(format!(
            "domain traffic change for '{}' requires explicit approval",
            domain.hostname
        )));
    }
    if !domain.is_routable() {
        return Err(CoreError::Deployment(format!(
            "domain '{}' is not routable",
            domain.hostname
        )));
    }
    if !matches!(delivery.dns, DnsState::Propagated) {
        return Err(CoreError::Deployment(format!(
            "domain '{}' has no propagated DNS; traffic not attached",
            domain.hostname
        )));
    }
    match &delivery.tls {
        CertificateState::Failed { reason } => {
            return Err(CoreError::Deployment(format!(
                "certificate issuance failed for '{}': {reason}. {CERTIFICATE_FAILURE_RECOVERY}",
                domain.hostname
            )));
        }
        state if !state.is_issued() => {
            return Err(CoreError::Deployment(format!(
                "domain '{}' has no issued TLS certificate; traffic not attached",
                domain.hostname
            )));
        }
        _ => {}
    }
    if !deployment.is_promoted() {
        return Err(CoreError::Deployment(format!(
            "cannot route '{}' to unpromoted deployment {} ({:?})",
            domain.hostname, deployment.id, deployment.phase
        )));
    }
    delivery.deployment_id = Some(deployment.id);
    delivery.traffic_attached = true;
    delivery.detail = Some(format!(
        "traffic attached to promoted deployment {} by {}",
        deployment.id, approval.approver
    ));
    Ok(())
}
