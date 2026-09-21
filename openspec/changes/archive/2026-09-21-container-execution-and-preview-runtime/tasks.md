# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every runtime, workspace, preview, log, health, and controller path to
  executor calls, persisted state, limits, cleanup, and evidence.
  (`labrys-core` runtime/workspace/observability/controller read; impact map in
  proposal `BFS Impact Map`.)
- [x] Add executor/process/fixture skeletons and container integration harness;
  label compile-only work `SKELETON_READY`.
  (Implemented directly as behavior with tests; no skeleton stage remained.)
- [x] Confirm persistence, runtime, preview, and security requirements agree.
  (Core `evaluate_health`, `SandboxLimits`, `WebPreview` gating reused as the
  authority; control plane adds I/O only.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement Docker build/run/stop/inspect/log/health operations with
  structured arguments, timeouts, cancellation, and redacted diagnostics.
  (`crates/labrys-control-plane/src/runtime.rs`: `ContainerExecutor`,
  `DockerExecutor`, `UnavailableExecutor`, `probe_health`; no shell on path.)
- [x] Enforce sandbox CPU, memory, filesystem, network, capability, and PID
  limits before scheduling and record the effective limits.
  (`enforce_limits_before_schedule`, `docker_run_args`, `EffectiveLimits`.)
- [x] Implement isolated session workspaces, preview startup, health gating,
  expiry/revocation, cleanup, and Expo transport metadata.
  (`crates/labrys-control-plane/src/preview.rs`: `WorkspaceRoot`,
  `PreviewManager`, `PgExecutionStore`, `PgPreviewStore`, migration
  `20260921000001_execution_and_previews.sql`.)
- [x] Wire controllers and durable jobs to runtime observations and separated
  platform logs.
  (`prospective_phase` maps platform health to `ResourcePhase`; preview
  transitions emit platform-actor events, separated runtime logs, and
  health/build-test evidence through the durable stores.)
- [x] Add container integration tests and explicit unavailable-runtime checks.
  (`crates/labrys-control-plane/tests/container_execution.rs`: 17 tests —
  10 pure unit, 3 PostgreSQL-backed, 4 Docker-backed; unavailable runtime
  asserted as blocker, never simulated.)

## 3. BFS — Cross-surface regression and completeness

- [x] Test concurrent sessions, failed builds, timeouts, health failures,
  expiry, cancellation, cleanup, and redaction across callers.
  (Two-container independence, `RUN sleep 30` timeout cancellation, unhealthy
  container yields no URL, expiry revokes persisted access, idempotent
  cleanup, secret redaction in execution rows.)
- [x] Verify no development process is promoted as production and no agent
  claim can establish health.
  (`verify_production_artifact` rejects development; `ContainerExecutor` has
  no agent-claim input; bare TCP accept reports no signal.)
- [x] Re-run core, persistence, migration, and runtime suites; record Docker
  availability and remaining release gaps.
  (Full workspace suite green: 156 core + 12 lib + 8 config + 17 container
  execution + 17 control-plane + 6 redaction; Docker 29.5.3 present,
  `node:24-alpine` workload healthy under default sandbox; registries,
  TLS/DNS, public API, CLI binary, dashboard remain later changes.)

## 4. Verification

- [x] Run format, lint, build, unit, container integration, security, Gate, and
  strict OpenSpec validation.
  (`cargo fmt --check`, `clippy -D warnings`, `cargo build --workspace`,
  `cargo test --workspace` with throwaway PostgreSQL 16 + real Docker,
  `cargo audit` exit 0, `driftwatchdog gate`, `openspec validate --strict`.)
- [x] Inspect the diff and confirm unrelated work is untouched.
  (Only this change's implementation, tests, migration, archive, and promoted
  specs staged; pre-existing ROADMAP/phase 5–7 planning work untouched.)
