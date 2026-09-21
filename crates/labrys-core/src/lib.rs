//! Labrys control-plane contracts.
//!
//! Pure, deterministic models for the platform's capability specs, one module
//! per promoted spec:
//!
//! - [`application`], [`state`], [`environment`], [`manifest`], [`origin`],
//!   [`repository`]: `application-model` — one canonical Application for every
//!   origin, environment isolation, and separately persisted desired and
//!   observed state.
//! - [`inspector`]: `project-inspector` — deterministic understanding,
//!   evidence/conflict handling, and approval-gated adoption plans.
//! - [`agent`]: `agent-platform` — versioned backend protocol, normalized
//!   events, session policy, approvals, and secret references.
//! - [`runtime`]: `runtime-platform` — adapter detection, dev/production
//!   profiles, OCI boundaries, and sandbox limits.
//! - [`workspace`]: `preview-platform` — per-session worktrees, diffs, merges,
//!   health-gated previews, Expo shares, and feedback.
//! - [`capability`]: `capability-platform` — capabilities, providers,
//!   bindings, modes, and sealed secrets.
//! - [`controller`]: `reconciliation` — controllers, idempotent jobs, resource
//!   lifecycle, and platform-only health aggregation.
//! - [`provider`]: `provider-registry-and-domain-adapters` — scoped provider
//!   operations, approval-gated destructive actions, digest-verified OCI
//!   delivery, and separately observable DNS/TLS/traffic attachment.
//! - [`deployment`]: `deployment-platform` — first-class OCI deployments,
//!   health-gated promotion, domains, and explicit rollback boundaries.
//! - [`observability`]: `verification-and-observability` — attributable events,
//!   separated logs, immutable audit chain, usage, and independent
//!   verification.
//! - [`surfaces`], [`plugin`]: `platform-surfaces` — CLI and dashboard
//!   boundaries and the versioned JSON-RPC plugin protocol with SDK examples.
//!
//! The platform database is authoritative for imported projects; `labrys.yaml`
//! is an optional export/import manifest (see [`Manifest`]). Every module is
//! I/O-free: real persistence, containers, registries, and workers are
//! infrastructure built on these plans.

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
pub mod observability;
pub mod origin;
pub mod plugin;
pub mod provider;
pub mod repository;
pub mod runtime;
pub mod state;
pub mod surfaces;
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
    SECRET_ENVELOPE_LEGACY_XOR, SECRET_ENVELOPE_VERSION,
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
pub use manifest::{Manifest, RuntimeNotes};
pub use observability::{
    AuditLog, AuditRecord, Correlation, EventAction, EventActor, EventDraft, EventLog,
    EventResource, EventResult, EvidenceStore, FailureExplanation, Finding, HealthSnapshot,
    LogEntry, LogSource, LogStore, PlatformEvent, RetentionPolicy, Severity, StageResult,
    UsageLedger, UsageRecord, UsageTotals, VerificationEvidence, VerificationVerdict, Verifier,
    VerifierStage, OBSERVABILITY_CONTRACT_VERSION, REDACTION_MARKER,
};
pub use origin::Origin;
pub use plugin::{
    decode_response, encode_response, register_sdk_examples, sdk_examples, ExampleAgentPlugin,
    ExampleBindingProviderPlugin, ExampleCapabilityProviderPlugin, ExampleDeployPlugin,
    ExampleInspectorPlugin, ExampleRuntimePlugin, Handshake, Plugin, PluginFamily, PluginManifest,
    PluginProcess, PluginRegistry, PluginReport, PluginState, RpcError, RpcErrorCode, RpcRequest,
    RpcResponse, PLUGIN_PROTOCOL_VERSION,
};
pub use provider::{
    gate_registry_push, note_agent_claim_ignored, plan_traffic_attachment, sanitize_failure,
    verify_registry_digest, CertificateState, DnsState, DomainDelivery, ProviderAction,
    ProviderKind, ProviderObservation, ProviderOperation, RegistryArtifact,
    CERTIFICATE_FAILURE_RECOVERY, DIGEST_MISMATCH_RECOVERY, PROVIDER_CONTRACT_VERSION,
    PROVIDER_FAILURE_RECOVERY,
};
pub use repository::{ApplicationRepository, InMemoryApplicationRepository};
pub use runtime::{
    detect_runtime, enforce_run_usage, evaluate_health, prepare, simulate_build, BuildResult,
    BuildStatus, DetectedRuntime, DotNetAdapter, Endpoint, ExpoAdapter, FrameworkConventions,
    GenericAdapter, HealthStatus, NetworkMode, NodeAdapter, PythonAdapter, RuntimeAdapter,
    RuntimeConfig, RuntimeKind, RuntimeProfile, RustAdapter, SandboxLimits, SupportTier,
};
pub use state::{DesiredState, ObservedState, ObservedStatus};
pub use surfaces::{
    attach_secret_rows, authorize_dashboard_write, render_dashboard, Attribution, Cli, CliActor,
    CliCommand, CliRequest, CliResponse, DashboardEntry, DashboardSection, DashboardView,
    PlatformState, SecretRow,
};

pub use workspace::{
    CanonicalRepository, ExpoShare, ExpoShareStatus, ExpoTransport, Feedback, MergeRequest,
    PreviewAccess, PreviewStatus, RollbackInfo, SessionWorkspace, WebPreview, WorkspaceDiff,
    WorkspaceRegistry, WorktreeStatus,
};
