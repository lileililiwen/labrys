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
pub mod capability;
pub mod controller;
pub mod deployment;
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
pub use capability::{
    generic_auth_binding, generic_file_storage_binding, generic_object_storage_binding,
    generic_postgres_binding, resolve_binding, Binding, BindingKind, BoundCapability, Capability,
    CapabilityMode, EnvContract, EnvSource, InjectionGrant, MasterKey, ModeTransition, Provider,
    ResourceRef, SealedSecret, SecretAction, SecretAudit, SecretStore, CAPABILITY_CONTRACT_VERSION,
};
pub use controller::{
    aggregate_health, redact_reason, ApplicationController, CapabilityController, CheckSource,
    Controller, DeploymentController, Drift, DriftKind, EnqueueOutcome, FailureRecord, HealthCheck,
    HealthReport, HealthState, IdempotencyKey, Job, JobAction, JobQueue, JobStatus,
    ObservationSource, PreviewController, Reconciliation, Resource, ResourceController,
    ResourcePhase, ResourceTransition, RetryPolicy, RECONCILIATION_CONTRACT_VERSION,
};
pub use deployment::{
    attach_domain, build_image, execute_rollback, plan_rollback, select_rollback_target,
    Deployment, DeploymentLog, DeploymentPhase, DeploymentTransition, Domain, Image, ImageSource,
    LogLevel, Revision, RollbackKind, RollbackOutcome, RollbackPlan, Rollout,
    CONFIGURATION_DATA_WARNING, DATABASE_DATA_WARNING, DEPLOYMENT_CONTRACT_VERSION,
};
pub use environment::{Environment, EnvironmentKind};
pub use error::{CoreError, Result};
pub use ids::{
    AgentSessionId, ApplicationId, BindingId, DeploymentId, DomainId, EnvironmentId, FeedbackId,
    JobId, PreviewId, ResourceId, WorkspaceId,
};
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
