//! Labrys durable control plane.
//!
//! This crate turns the pure `labrys-core` contracts into an executable,
//! durable control plane: PostgreSQL/SQLx persistence adapters, versioned
//! migrations, transactional orchestration, redacted evidence writers, a
//! recoverable reconciliation worker, and the control-plane process
//! configuration.
//!
//! Boundaries (see `control-plane-persistence-and-worker` and
//! `container-execution-and-preview-runtime`):
//! - `labrys-core` stays the authoritative, I/O-free contract and policy layer;
//!   this crate depends on it, never the reverse.
//! - Desired state and observed state are persisted separately and reconciled by
//!   the worker; an agent completion claim is recorded as non-authoritative and
//!   can never promote readiness.
//! - Secret values are redacted and rejected *before* SQL execution, not only
//!   when read back.
//! - The executable API ([`api`]) and `labrys` CLI (typed [`client`] plus the
//!   `labrys` binary) expose every lifecycle command over authenticated,
//!   idempotent, attributable envelopes; handlers validate, enqueue, and
//!   report rather than performing provider work inline.
//! - Container execution runs through the bounded [`runtime`] executor (real
//!   Docker I/O behind [`runtime::ContainerExecutor`], never a shell) and
//!   health-gated [`preview`] lifecycles; capability provisioning, OCI
//!   registry delivery, and DNS/TLS/traffic attachment run through the
//!   [`providers`] adapters (local test doubles by default, real provider I/O
//!   behind [`providers::ProviderAdapter`]/[`providers::OciRegistry`]/
//!   [`providers::DomainDeliveryAdapter`], never a shell); the dashboard
//!   remains a later change and is not introduced here.

pub mod api;
pub mod client;
pub mod config;
pub mod db;
pub mod error;
pub mod jobs;
pub mod mapping;
pub mod observability;
pub mod preview;
pub mod providers;
pub mod redact;
pub mod repos;
pub mod runtime;
pub mod worker;

pub use api::{router as api_router, ApiConfig, ApiState, API_VERSION};
pub use client::{exit_for, recovery_of, ControlPlaneClient, ExitCode};
pub use config::Config;
pub use db::{connect, run_migrations, Pool};
pub use error::{ControlPlaneError, Result};
pub use jobs::{ClaimedJob, PgJobQueue};
pub use observability::{PgAuditLog, PgEventStore, PgEvidenceStore, PgLogStore, PgUsageLedger};
pub use preview::{
    ExecutionRecord, PgExecutionStore, PgPreviewStore, PreviewManager, PreviewRow, StartedPreview,
    WorkspaceRoot,
};
pub use providers::{
    DomainDeliveryAdapter, DomainDeliveryRecord, InjectedFailure, LocalDomainDelivery,
    LocalTestAdapter, LocalTestRegistry, OciRegistry, ProviderAdapter, ProviderExecution,
    ProviderJobDispatcher, ProviderOperationRecord, ProviderRuntime, RegistryDeliveryRecord,
};
pub use repos::{PgApplicationStore, PgCapabilityStore, PgDeploymentStore, PgResourceStore};
pub use runtime::{
    docker_build_args, docker_run_args, enforce_limits_before_schedule, probe_health,
    prospective_phase, verify_production_artifact, ContainerExecutor, DockerExecutor,
    EffectiveLimits, ExecutionIdentity, RunningContainer, RuntimeAvailability, UnavailableExecutor,
};
pub use worker::{
    DispatchOutcome, JobDispatcher, NoopDispatcher, ScriptedDispatcher, TickOutcome, Worker,
    WorkerStats,
};

/// Version of the durable control-plane schema this crate ships.
pub const CONTROL_PLANE_SCHEMA_VERSION: u32 = 1;
