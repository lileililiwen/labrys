//! Versioned agent protocol, platform policy, and adapter boundary.
//!
//! Covers requirement `agent-platform`: native and third-party backends share
//! one session, prompt, interrupt, and resume contract with a normalized event
//! vocabulary; high-risk actions wait for human approval; agent context carries
//! secret references, never secret values.
//!
//! Policy is enforced at the platform boundary ([`AgentSession`]), never inside
//! a third-party adapter. Tool precedence is Platform API → framework tool →
//! generic tool → raw shell ([`ToolTier`]).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::{AgentSessionId, ApplicationId};
use crate::inspector::ExplicitApproval;

/// Version of the [`AgentBackend`] contract this crate implements.
///
/// Adapters reporting any other version are rejected with
/// [`CoreError::ProtocolVersion`] before they can emit events.
pub const AGENT_PROTOCOL_VERSION: u32 = 1;

/// Canonical event names emitted by the native backend.
///
/// External adapters must normalize their backend-specific names to exactly
/// these values; consumers never match on backend-specific names.
pub const NORMALIZED_EVENT_NAMES: [&str; 14] = [
    "planning",
    "file_changed",
    "command_run",
    "capability_requested",
    "build",
    "preview",
    "deployment",
    "verification",
    "approval_requested",
    "denied",
    "interrupted",
    "resumed",
    "timed_out",
    "completed",
];

/// Which backend produced an event. Recorded on every [`AgentEvent`] so
/// consumers can attribute behavior without depending on backend-specific
/// event names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentBackendKind {
    Native,
    External,
}

/// Identity of the backend behind a session or event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendIdentity {
    pub kind: AgentBackendKind,
    pub name: String,
}

impl BackendIdentity {
    pub fn native() -> Self {
        Self {
            kind: AgentBackendKind::Native,
            name: "labrys-native".to_string(),
        }
    }

    pub fn external(name: impl Into<String>) -> Self {
        Self {
            kind: AgentBackendKind::External,
            name: name.into(),
        }
    }
}

/// Normalized platform event vocabulary.
///
/// Design contract: planning, file changes, commands, capability requests,
/// builds, previews, deployments, verification, errors, and completion share
/// one vocabulary across backends. Lifecycle transitions (interrupt, resume,
/// timeout) and policy outcomes (approval requested, denied) are events too so
/// the stream stays replayable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEventKind {
    Planning,
    FileChanged,
    CommandRun,
    CapabilityRequested,
    Build,
    Preview,
    Deployment,
    Verification,
    ApprovalRequested,
    Denied,
    Interrupted,
    Resumed,
    TimedOut,
    Completed,
}

impl AgentEventKind {
    /// Canonical wire name shared by all backends.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::FileChanged => "file_changed",
            Self::CommandRun => "command_run",
            Self::CapabilityRequested => "capability_requested",
            Self::Build => "build",
            Self::Preview => "preview",
            Self::Deployment => "deployment",
            Self::Verification => "verification",
            Self::ApprovalRequested => "approval_requested",
            Self::Denied => "denied",
            Self::Interrupted => "interrupted",
            Self::Resumed => "resumed",
            Self::TimedOut => "timed_out",
            Self::Completed => "completed",
        }
    }
}

/// One normalized event in an agent session stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvent {
    pub sequence: u64,
    pub session_id: AgentSessionId,
    pub backend: BackendIdentity,
    pub kind: AgentEventKind,
    /// Human/tool-readable detail. Must never contain secret values; secret
    /// references (see [`SecretReference`]) are the only allowed form.
    pub detail: String,
}

impl AgentEvent {
    /// Canonical JSON serialization.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    /// Strict JSON deserialization.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Reference to a protected secret. The only secret shape allowed in agent
/// context; values travel exclusively through runtime injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    pub ref_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_hint: Option<String>,
}

impl SecretReference {
    pub fn new(ref_id: impl Into<String>) -> Self {
        Self {
            ref_id: ref_id.into(),
            redacted_hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.redacted_hint = Some(hint.into());
        self
    }
}

/// Platform tool precedence: Platform API → framework tool → generic tool →
/// raw shell. Lower rank wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolTier {
    PlatformApi,
    FrameworkTool,
    GenericTool,
    RawShell,
}

impl ToolTier {
    fn rank(self) -> u8 {
        match self {
            Self::PlatformApi => 0,
            Self::FrameworkTool => 1,
            Self::GenericTool => 2,
            Self::RawShell => 3,
        }
    }
}

/// Selects the highest-precedence tier from the candidates.
///
/// Returns `None` when no candidate is offered. Deterministic: ties keep the
/// first occurrence.
pub fn select_preferred_tool(candidates: &[ToolTier]) -> Option<ToolTier> {
    candidates.iter().copied().min_by_key(|tier| tier.rank())
}

/// Actions an agent may request. The high-risk variants always require a
/// recorded human approval before execution (see [`Policy`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionKind {
    ReadOnly,
    PreviewDeploy,
    ProductionDeploy,
    DestructiveResource,
    DestructiveMigration,
    SecretReplacement,
    DomainChange,
    BillingChange,
    ExternalWrite,
}

impl ActionKind {
    /// True for the spec-mandated approval set: production deploys,
    /// destructive resource actions, destructive migrations, secret
    /// replacement, domain changes, billing changes, and external writes.
    pub fn requires_approval(&self) -> bool {
        match self {
            Self::ReadOnly | Self::PreviewDeploy => false,
            Self::ProductionDeploy
            | Self::DestructiveResource
            | Self::DestructiveMigration
            | Self::SecretReplacement
            | Self::DomainChange
            | Self::BillingChange
            | Self::ExternalWrite => true,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only inspection",
            Self::PreviewDeploy => "preview deployment",
            Self::ProductionDeploy => "production deployment",
            Self::DestructiveResource => "destructive resource action",
            Self::DestructiveMigration => "destructive migration",
            Self::SecretReplacement => "secret replacement",
            Self::DomainChange => "domain change",
            Self::BillingChange => "billing change",
            Self::ExternalWrite => "external write",
        }
    }
}

/// Execution context for one agent session. Owned by the platform and handed
/// to every backend unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContext {
    pub application_id: ApplicationId,
    pub environment_name: String,
    /// True when the session targets production. Raw shell tools are denied
    /// there regardless of approvals.
    pub production: bool,
    pub principal: String,
}

impl AgentContext {
    pub fn new(
        application_id: ApplicationId,
        environment_name: impl Into<String>,
        production: bool,
        principal: impl Into<String>,
    ) -> Self {
        Self {
            application_id,
            environment_name: environment_name.into(),
            production,
            principal: principal.into(),
        }
    }
}

/// One agent prompt with its policy-relevant request envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    /// Prompt text. Must not contain protected secret values.
    pub text: String,
    #[serde(default)]
    pub secret_refs: Vec<SecretReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_action: Option<ActionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolTier>,
}

impl AgentInput {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            secret_refs: Vec::new(),
            requested_action: None,
            tool: None,
        }
    }

    pub fn with_action(mut self, action: ActionKind) -> Self {
        self.requested_action = Some(action);
        self
    }

    pub fn with_tool(mut self, tool: ToolTier) -> Self {
        self.tool = Some(tool);
        self
    }

    pub fn with_secret_ref(mut self, secret_ref: SecretReference) -> Self {
        self.secret_refs.push(secret_ref);
        self
    }
}

/// Capability request envelope: what the agent wants, never the secret value
/// that authorizes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequest {
    pub capability: String,
    pub action: String,
    #[serde(default)]
    pub secret_refs: Vec<SecretReference>,
    pub reason: String,
}

impl CapabilityRequest {
    /// Renders the event detail; contains only reference identifiers.
    pub fn to_event_detail(&self) -> String {
        let refs = self
            .secret_refs
            .iter()
            .map(|s| s.ref_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "capability '{}' action '{}' refs:[{refs}] reason: {}",
            self.capability, self.action, self.reason
        )
    }
}

/// Resource operation envelope for controller-bound resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceOperation {
    Read,
    Create,
    Update,
    Delete,
}

/// Resource request envelope: identity plus operation, never embedded
/// credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequest {
    pub resource: String,
    pub operation: ResourceOperation,
    #[serde(default)]
    pub secret_refs: Vec<SecretReference>,
    pub reason: String,
}

impl ResourceRequest {
    /// Renders the event detail; contains only reference identifiers.
    pub fn to_event_detail(&self) -> String {
        let refs = self
            .secret_refs
            .iter()
            .map(|s| s.ref_id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "resource '{}' operation '{:?}' refs:[{refs}] reason: {}",
            self.resource, self.operation, self.reason
        )
    }
}

/// Platform approval request created when a high-risk action is proposed.
/// Execution cannot start until [`ExplicitApproval`] resolves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: AgentSessionId,
    pub action: ActionKind,
    pub reason: String,
    pub requested_by: String,
}

/// Lifecycle status of an agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionStatus {
    Active,
    Interrupted,
    Completed,
    TimedOut,
}

/// Platform-owned agent session: the policy boundary.
///
/// The session owns the normalized event log, sequence numbers, timeouts, and
/// the protected secret values used exclusively for leak scanning. Backends
/// draft candidate events; only this session appends them after policy checks.
pub struct AgentSession {
    id: AgentSessionId,
    context: AgentContext,
    backend: BackendIdentity,
    status: SessionStatus,
    events: Vec<AgentEvent>,
    next_sequence: u64,
    deadline: Option<DateTime<Utc>>,
    pending_approvals: Vec<ApprovalRequest>,
    approval_counter: u64,
    /// Protected secret values, platform-held for leak scanning. Never
    /// serialized into events, logs, or prompts.
    protected_secrets: Vec<String>,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("id", &self.id)
            .field("context", &self.context)
            .field("backend", &self.backend)
            .field("status", &self.status)
            .field("events", &self.events)
            .field("next_sequence", &self.next_sequence)
            .field("deadline", &self.deadline)
            .field("pending_approvals", &self.pending_approvals)
            .field("protected_secrets", &"[redacted]")
            .finish()
    }
}

impl AgentSession {
    /// Opens a session after validating the backend protocol version.
    pub fn open(
        backend: &dyn AgentBackend,
        context: AgentContext,
        protected_secrets: Vec<String>,
    ) -> Result<Self> {
        ensure_protocol_version(backend.protocol_version())?;
        Ok(Self {
            id: AgentSessionId::new(),
            context,
            backend: backend.identity(),
            status: SessionStatus::Active,
            events: Vec::new(),
            next_sequence: 0,
            deadline: None,
            pending_approvals: Vec::new(),
            approval_counter: 0,
            protected_secrets,
        })
    }

    pub fn id(&self) -> AgentSessionId {
        self.id
    }

    pub fn context(&self) -> &AgentContext {
        &self.context
    }

    pub fn backend(&self) -> &BackendIdentity {
        &self.backend
    }

    pub fn status(&self) -> SessionStatus {
        self.status
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn pending_approvals(&self) -> &[ApprovalRequest] {
        &self.pending_approvals
    }

    /// Sets an absolute prompt deadline; prompts past it fail with
    /// [`CoreError::Timeout`].
    pub fn set_deadline(&mut self, deadline: DateTime<Utc>) {
        self.deadline = Some(deadline);
    }

    /// Runs one prompt through policy checks, then the backend adapter.
    ///
    /// Fails with [`CoreError::SessionState`] when not active,
    /// [`CoreError::Timeout`] past the deadline, [`CoreError::SecretLeak`]
    /// when the prompt carries a protected value, [`CoreError::PolicyDenied`]
    /// for denied tools, and [`CoreError::ApprovalRequired`] when a human
    /// approval must be recorded first.
    pub fn prompt(
        &mut self,
        backend: &mut dyn AgentBackend,
        input: &AgentInput,
        now: DateTime<Utc>,
    ) -> Result<Vec<AgentEvent>> {
        ensure_protocol_version(backend.protocol_version())?;
        self.require_status(SessionStatus::Active, "prompt")?;
        self.check_timeout(now)?;
        self.scan_text(&input.text)?;
        for secret_ref in &input.secret_refs {
            self.scan_text(&secret_ref.ref_id)?;
        }
        if let Err(denied) = Policy::authorize(
            input.requested_action.as_ref(),
            input.tool,
            self.context.production,
        ) {
            let _ = self
                .append_system_event(AgentEventKind::Denied, format!("prompt denied: {denied}"));
            return Err(denied);
        }
        if let Some(action) = input
            .requested_action
            .as_ref()
            .filter(|a| a.requires_approval())
        {
            return Err(self.require_approval(action));
        }
        let candidates = backend.draft_events(&self.context, input, self.next_sequence)?;
        let mut appended = Vec::with_capacity(candidates.len());
        for (kind, detail) in candidates {
            appended.push(self.append_normalized_impl(kind, detail, backend.identity())?);
        }
        Ok(appended)
    }

    /// Interrupts a running session. The event log stays replayable.
    pub fn interrupt(&mut self) -> Result<AgentEvent> {
        self.require_status(SessionStatus::Active, "interrupt")?;
        self.status = SessionStatus::Interrupted;
        self.append_system_event(
            AgentEventKind::Interrupted,
            "session interrupted".to_string(),
        )
    }

    /// Resumes an interrupted session and returns the full event log for
    /// replay so a new worker continues exactly where the log ends.
    pub fn resume(&mut self) -> Result<Vec<AgentEvent>> {
        self.require_status(SessionStatus::Interrupted, "resume")?;
        self.status = SessionStatus::Active;
        self.append_system_event(AgentEventKind::Resumed, "session resumed".to_string())?;
        Ok(self.events.clone())
    }

    /// Completes a session; further prompts are rejected.
    pub fn complete(&mut self) -> Result<AgentEvent> {
        self.require_status(SessionStatus::Active, "complete")?;
        self.status = SessionStatus::Completed;
        self.append_system_event(AgentEventKind::Completed, "session completed".to_string())
    }

    /// Replays one persisted event, e.g. when restoring a resumable session.
    /// Rejects out-of-order sequences and foreign sessions or backends.
    pub fn replay_event(&mut self, event: AgentEvent) -> Result<()> {
        if event.session_id != self.id {
            return Err(CoreError::SessionState(format!(
                "event belongs to session {}, not {}",
                event.session_id, self.id
            )));
        }
        if event.backend != self.backend {
            return Err(CoreError::SessionState(format!(
                "event backend '{}' does not match session backend '{}'",
                event.backend.name, self.backend.name
            )));
        }
        if event.sequence != self.next_sequence {
            return Err(CoreError::SessionState(format!(
                "event out of order: got sequence {}, expected {}",
                event.sequence, self.next_sequence
            )));
        }
        self.scan_text(&event.detail)?;
        self.next_sequence += 1;
        self.events.push(event);
        Ok(())
    }

    /// Resolves a pending approval with an explicit human decision.
    pub fn resolve_approval(
        &mut self,
        request_id: &str,
        approval: &ExplicitApproval,
    ) -> Result<AgentEvent> {
        let position = self
            .pending_approvals
            .iter()
            .position(|r| r.id == request_id)
            .ok_or_else(|| {
                CoreError::ApprovalRequired(format!("unknown approval request '{request_id}'"))
            })?;
        if !approval.approved {
            let event = self.append_system_event(
                AgentEventKind::Denied,
                format!("approval {request_id} denied by {}", approval.approver),
            )?;
            self.pending_approvals.remove(position);
            return Ok(event);
        }
        if approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(
                "approval must name an approver".to_string(),
            ));
        }
        let event = self.append_system_event(
            AgentEventKind::Verification,
            format!("approval {request_id} granted by {}", approval.approver),
        )?;
        self.pending_approvals.remove(position);
        Ok(event)
    }

    fn require_approval(&mut self, action: &ActionKind) -> CoreError {
        self.approval_counter += 1;
        let request = ApprovalRequest {
            id: format!("apr_{}_{}", self.id, self.approval_counter),
            session_id: self.id,
            action: action.clone(),
            reason: format!(
                "{} requires human approval before execution",
                action.label()
            ),
            requested_by: self.backend.name.clone(),
        };
        let detail = format!("approval {} requested: {}", request.id, request.reason);
        self.pending_approvals.push(request);
        // The request itself is observable even though execution is blocked.
        let _ = self.append_system_event(AgentEventKind::ApprovalRequested, detail);
        CoreError::ApprovalRequired(format!(
            "{} blocked until a human approval is recorded",
            action.label()
        ))
    }

    fn require_status(&self, expected: SessionStatus, operation: &str) -> Result<()> {
        if self.status == expected {
            Ok(())
        } else {
            Err(CoreError::SessionState(format!(
                "cannot {operation} while session is {:?} (expected {:?})",
                self.status, expected
            )))
        }
    }

    fn check_timeout(&mut self, now: DateTime<Utc>) -> Result<()> {
        if let Some(deadline) = self.deadline {
            if now > deadline {
                self.status = SessionStatus::TimedOut;
                let _ = self.append_system_event(
                    AgentEventKind::TimedOut,
                    "agent prompt timed out".to_string(),
                );
                return Err(CoreError::Timeout(
                    "agent prompt exceeded its deadline".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn scan_text(&self, text: &str) -> Result<()> {
        for protected in &self.protected_secrets {
            if !protected.is_empty() && text.contains(protected) {
                return Err(CoreError::SecretLeak(
                    "agent context contains a protected secret value; use a secret reference instead"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }

    fn append_system_event(&mut self, kind: AgentEventKind, detail: String) -> Result<AgentEvent> {
        self.append_normalized_impl(kind, detail, self.backend.clone())
    }

    fn append_normalized_impl(
        &mut self,
        kind: AgentEventKind,
        detail: String,
        backend: BackendIdentity,
    ) -> Result<AgentEvent> {
        self.scan_text(&detail)?;
        let event = AgentEvent {
            sequence: self.next_sequence,
            session_id: self.id,
            backend,
            kind,
            detail,
        };
        self.next_sequence += 1;
        self.events.push(event.clone());
        Ok(event)
    }
}

/// Platform policy evaluated at the session boundary.
pub struct Policy;

impl Policy {
    /// Denies raw shell tools in production; they must be replaced by
    /// platform, framework, or generic tools. Everything else passes here;
    /// approval gating happens in [`AgentSession::prompt`].
    pub fn authorize(
        action: Option<&ActionKind>,
        tool: Option<ToolTier>,
        production: bool,
    ) -> Result<()> {
        let _ = action;
        if production && tool == Some(ToolTier::RawShell) {
            return Err(CoreError::PolicyDenied(
                "raw shell is denied in production; use a platform, framework, or generic tool"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// Rejects adapters that do not speak [`AGENT_PROTOCOL_VERSION`].
pub fn ensure_protocol_version(found: u32) -> Result<()> {
    if found == AGENT_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(CoreError::ProtocolVersion {
            found,
            expected: AGENT_PROTOCOL_VERSION,
        })
    }
}

/// One session, prompt, interrupt, and resume contract shared by native and
/// third-party backends. Adapters draft candidate normalized events; the
/// platform session appends them after policy checks.
pub trait AgentBackend {
    fn identity(&self) -> BackendIdentity;

    fn protocol_version(&self) -> u32 {
        AGENT_PROTOCOL_VERSION
    }

    /// Drafts candidate `(kind, detail)` pairs for one prompt. Details must be
    /// free of secret values; the session re-scans before appending.
    fn draft_events(
        &mut self,
        context: &AgentContext,
        input: &AgentInput,
        start_sequence: u64,
    ) -> Result<Vec<(AgentEventKind, String)>>;
}

/// Reference phases of the native agent: Plan → Act → Observe → Verify → Retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePhase {
    Plan,
    Act,
    Observe,
    Verify,
    Retry,
}

impl NativePhase {
    fn as_event_kind(self) -> AgentEventKind {
        match self {
            Self::Plan => AgentEventKind::Planning,
            Self::Act => AgentEventKind::CommandRun,
            Self::Observe => AgentEventKind::FileChanged,
            Self::Verify => AgentEventKind::Verification,
            Self::Retry => AgentEventKind::Planning,
        }
    }

    fn detail(self, prompt: &str) -> String {
        match self {
            Self::Plan => format!("native plan for prompt: {prompt}"),
            Self::Act => format!("native act for prompt: {prompt}"),
            Self::Observe => format!("native observe for prompt: {prompt}"),
            Self::Verify => format!("native verify for prompt: {prompt}"),
            Self::Retry => format!("native retry for prompt: {prompt}"),
        }
    }
}

/// Native reference agent: implements Plan → Act → Observe → Verify → Retry
/// directly in the canonical vocabulary.
pub struct NativeAgent {
    identity: BackendIdentity,
}

impl NativeAgent {
    pub fn new() -> Self {
        Self {
            identity: BackendIdentity::native(),
        }
    }

    /// Runs the full reference flow as one prompt response.
    pub fn reference_flow(&self) -> Vec<NativePhase> {
        vec![
            NativePhase::Plan,
            NativePhase::Act,
            NativePhase::Observe,
            NativePhase::Verify,
        ]
    }
}

impl Default for NativeAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBackend for NativeAgent {
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn draft_events(
        &mut self,
        _context: &AgentContext,
        input: &AgentInput,
        _start_sequence: u64,
    ) -> Result<Vec<(AgentEventKind, String)>> {
        Ok(self
            .reference_flow()
            .into_iter()
            .map(|phase| (phase.as_event_kind(), phase.detail(&input.text)))
            .collect())
    }
}

/// External process-hosted agent adapter (e.g. an OpenCode child process).
///
/// The adapter owns the transport (`command`) and the backend name; the
/// platform owns policy. Backend-specific emission names are normalized here
/// so consumers only ever observe [`AgentEventKind`].
pub struct ExternalProcessAgent {
    identity: BackendIdentity,
    protocol_version: u32,
    command: Vec<String>,
    scripted: Vec<(AgentEventKind, String)>,
}

impl ExternalProcessAgent {
    pub fn new(name: impl Into<String>, protocol_version: u32, command: Vec<String>) -> Self {
        Self {
            identity: BackendIdentity::external(name),
            protocol_version,
            command,
            scripted: Vec::new(),
        }
    }

    /// Test/support hook: scripted emissions returned for the next prompt.
    pub fn with_scripted(mut self, scripted: Vec<(AgentEventKind, String)>) -> Self {
        self.scripted = scripted;
        self
    }

    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Normalizes one backend-specific emission name to the canonical
    /// vocabulary. Unknown names are contract violations.
    pub fn normalize(&self, raw_name: &str, detail: String) -> Result<(AgentEventKind, String)> {
        let normalized = raw_name
            .trim()
            .to_ascii_lowercase()
            .replace(['.', '-'], "_");
        // Strip vendor prefixes (`opencode_plan` -> `plan`) by trying the
        // full trailing segment first, then progressively shorter suffixes,
        // so multi-word canonical names (`file_changed`) match before any
        // single-word suffix is considered.
        let parts: Vec<&str> = normalized.split('_').collect();
        let mut kind = None;
        for start in 0..parts.len() {
            let candidate = parts[start..].join("_");
            let found = match candidate.as_str() {
                "planning" | "plan" => Some(AgentEventKind::Planning),
                "file_changed" | "file" | "files" | "observe" => Some(AgentEventKind::FileChanged),
                "command_run" | "command" | "act" | "exec" => Some(AgentEventKind::CommandRun),
                "capability_requested" | "capability" => Some(AgentEventKind::CapabilityRequested),
                "build" => Some(AgentEventKind::Build),
                "preview" => Some(AgentEventKind::Preview),
                "deployment" | "deploy" => Some(AgentEventKind::Deployment),
                "verification" | "verify" => Some(AgentEventKind::Verification),
                "completed" | "complete" | "done" => Some(AgentEventKind::Completed),
                _ => None,
            };
            if found.is_some() {
                kind = found;
                break;
            }
        }
        match kind {
            Some(kind) => Ok((kind, detail)),
            None => Err(CoreError::SessionState(format!(
                "unknown backend event '{raw_name}' from '{}'; consumers require the normalized vocabulary",
                self.identity.name
            ))),
        }
    }
}

impl AgentBackend for ExternalProcessAgent {
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn protocol_version(&self) -> u32 {
        self.protocol_version
    }

    fn draft_events(
        &mut self,
        _context: &AgentContext,
        _input: &AgentInput,
        _start_sequence: u64,
    ) -> Result<Vec<(AgentEventKind, String)>> {
        ensure_protocol_version(self.protocol_version)?;
        if self.scripted.is_empty() {
            Ok(vec![(
                AgentEventKind::Planning,
                format!("external '{}' planned the prompt", self.identity.name),
            )])
        } else {
            Ok(std::mem::take(&mut self.scripted))
        }
    }
}
