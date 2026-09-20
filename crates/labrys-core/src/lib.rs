//! Labrys control-plane foundation.
//!
//! Canonical [`Application`] model shared by generated and imported projects.
//! Covers requirement `application-model`: one model for all origins,
//! environment-isolated operational state, and optional portable manifests.
//!
//! The platform database is authoritative for imported projects; `labrys.yaml`
//! is an optional export/import manifest (see [`Manifest`]).

pub mod agent;
pub mod application;
pub mod environment;
pub mod error;
pub mod ids;
pub mod inspector;
pub mod manifest;
pub mod origin;
pub mod repository;
pub mod runtime;
pub mod state;
pub mod workspace;

pub use agent::{
    ensure_protocol_version, select_preferred_tool, ActionKind, AgentBackend, AgentContext,
    AgentEvent, AgentEventKind, AgentInput, AgentSession, ApprovalRequest, BackendIdentity,
    CapabilityRequest, ExternalProcessAgent, NativeAgent, NativePhase, Policy, ResourceOperation,
    ResourceRequest, SecretReference, SessionStatus, ToolTier, AGENT_PROTOCOL_VERSION,
    NORMALIZED_EVENT_NAMES,
};
pub use application::{Application, ApplicationStatus, AuditMetadata, LifecycleCollections};
pub use environment::{Environment, EnvironmentKind};
pub use error::{CoreError, Result};
pub use ids::{AgentSessionId, ApplicationId, EnvironmentId, FeedbackId, PreviewId, WorkspaceId};
pub use inspector::{
    AdoptionItem, AdoptionItemKind, AdoptionPlan, ApplicationProfile, BroadChangeGate, ChangeStep,
    Evidence, ExplicitApproval, Fact, Inspector, InspectorPlugin, PluginFinding, ProjectSnapshot,
    Unknown, INSPECTION_ORDER,
};
pub use manifest::Manifest;
pub use origin::Origin;
pub use repository::{ApplicationRepository, InMemoryApplicationRepository};
pub use runtime::{
    detect_runtime, enforce_run_usage, evaluate_health, prepare, simulate_build, BuildResult,
    BuildStatus, DetectedRuntime, DotNetAdapter, Endpoint, ExpoAdapter, GenericAdapter,
    HealthStatus, NetworkMode, NodeAdapter, PythonAdapter, RuntimeAdapter, RuntimeConfig,
    RuntimeKind, RuntimeProfile, RustAdapter, SandboxLimits, SupportTier,
};
pub use state::{DesiredState, ObservedState, ObservedStatus};
pub use workspace::{
    CanonicalRepository, ExpoShare, ExpoShareStatus, ExpoTransport, Feedback, MergeRequest,
    PreviewAccess, PreviewStatus, RollbackInfo, SessionWorkspace, WebPreview, WorkspaceDiff,
    WorkspaceRegistry, WorktreeStatus,
};
