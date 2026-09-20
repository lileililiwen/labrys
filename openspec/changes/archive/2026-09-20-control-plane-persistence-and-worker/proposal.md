# Proposal: Durable control-plane persistence and reconciliation worker

## Why

Labrys currently has deterministic Rust contracts and in-memory test seams, but
no process can persist applications, desired state, observations, events, or
jobs after the process exits. The archived controller change describes a
PostgreSQL job boundary and worker, yet the current crate still has no SQLx
implementation, migrations, executable, or integration evidence. This gap
blocks every real Run → Preview → Capability → Deploy loop.

## What Changes

- Add a PostgreSQL/SQLx persistence adapter for the authoritative application,
  environment, desired/observed state, capability/resource, deployment,
  event/audit, and verification records already modeled by `labrys-core`.
- Add versioned migrations, transaction boundaries, optimistic generation
  checks, and redacted persistence for secret-bearing evidence.
- Add a durable job repository and a Rust worker that claims, retries, pauses,
  dead-letters, and resumes controller jobs without request-lifetime coupling.
- Add an executable control-plane process configuration and integration tests
  against an isolated PostgreSQL instance; keep Docker/container execution,
  registry/TLS providers, HTTP product API, CLI binary, and dashboard as later
  changes.

## BFS Impact Map

- **Capabilities and callers:** `ApplicationRepository`, controller
  reconciliation, `JobQueue`, `EventLog`, `AuditLog`, `EvidenceStore`, and
  `UsageLedger` gain durable adapters; canonical domain contracts remain
  reusable and I/O-free.
- **Data/persistence:** migrations must preserve environment isolation,
  generation/idempotency uniqueness, append-only audit ordering, redaction, and
  rollback-safe transaction semantics.
- **Process/runtime:** a worker must recover leases after interruption and
  never claim agent completion as readiness; no Docker daemon is introduced by
  this change.
- **Security/privacy:** secret values stay outside agent events, logs, audit,
  and database diagnostics; database credentials come from process
  configuration, not manifests or prompts.
- **Tests/evidence:** unit tests cover mapping and failure behavior;
  PostgreSQL integration tests cover restart/retry/concurrency and migration
  behavior. Passing model tests alone remains insufficient.
- **Unaffected by this change:** dashboard presentation, public HTTP/CLI
  command UX, container execution, OCI registry push, TLS issuance, provider
  implementations, and production deployment evidence.

## Capabilities

- `control-plane-persistence`
- `reconciliation-worker`

## Non-goals

- Do not implement container execution, image registries, domains/TLS, or
  external capability providers.
- Do not expose a public API or claim a production-ready control plane.
- Do not replace the pure deterministic core with database-dependent domain
  logic.
- Do not persist plaintext secrets or free-text credential-bearing errors.
