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
//!   health-gated [`preview`] lifecycles; capability provisioning runs through
//!   real provider adapters (managed PostgreSQL in [`postgres_provider`],
//!   filesystem buckets in [`storage_provider`]) or local test doubles when
//!   unconfigured, OCI registry delivery through the real [`oci_client`]
//!   behind [`providers::OciRegistry`], and DNS/TLS/traffic attachment through
//!   the real [`tls_dns`] delivery behind
//!   [`providers::DomainDeliveryAdapter`] (never a shell; credentials from
//!   process configuration, redacted before persistence); the operator
//!   console lives in `dashboard/` (Next.js over the versioned API) and CI,
//!   packaging, and tiered evidence live in `.github/workflows/ci.yml`,
//!   `scripts/`, and `docs/release.md`.

pub mod api;
pub mod client;
pub mod config;
pub mod db;
pub mod dispatch;
pub mod error;
pub mod jobs;
pub mod mapping;
pub mod observability;
pub mod oci_client;
pub mod postgres_provider;
pub mod preview;
pub mod providers;
pub mod redact;
pub mod repos;
pub mod runtime;
pub mod storage_provider;
pub mod tls_dns;
pub mod worker;

pub use api::{router as api_router, ApiConfig, ApiState, API_VERSION};
pub use client::{exit_for, recovery_of, ControlPlaneClient, ExitCode};
pub use config::{Config, RuntimeMode, DEFAULT_PREVIEW_TTL_SECS};
pub use db::{connect, run_migrations, Pool};
pub use dispatch::{
    identity_for_job, select_dispatcher, ExecutableDispatcher, SelectedDispatcher,
    UnavailableDispatcher, BLOCKER_RECOVERY, DISPATCHER_ACTOR,
};
pub use error::{ControlPlaneError, Result};
pub use jobs::{ClaimedJob, PgJobQueue};
pub use observability::{PgAuditLog, PgEventStore, PgEvidenceStore, PgLogStore, PgUsageLedger};
pub use oci_client::{
    oci_manifest, reference_parts, sha256_hex, OciRegistryClient, OCI_CONFIG_MEDIA_TYPE,
    OCI_MANIFEST_MEDIA_TYPE,
};
pub use postgres_provider::{PostgresProvisioner, ProvisionedCredential};
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
pub use storage_provider::{
    FileStorageProvisioner, FsBucketBackend, ObjectStorageProvisioner, ScopedKey,
    DEFAULT_KEY_TTL_SECS,
};
pub use tls_dns::{OpensslTlsIssuer, RealDomainDelivery, ResolvingDns};
pub use worker::{
    DispatchOutcome, JobDispatcher, NoopDispatcher, ScriptedDispatcher, TickOutcome, Worker,
    WorkerStats,
};

/// Version of the durable control-plane schema this crate ships.
pub const CONTROL_PLANE_SCHEMA_VERSION: u32 = 1;
