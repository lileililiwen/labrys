# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every daemon, worker, executor, preview, provider-operation, API
  finalization, smoke, and evidence path touched by dispatcher selection.
  (Daemon entry `bin/control-plane.rs`, `worker::JobDispatcher`/`NoopDispatcher`,
  `runtime::DockerExecutor`/`UnavailableExecutor`/`ExecutionIdentity`,
  `preview::PreviewManager`/`WorkspaceRoot`, `providers::ProviderJobDispatcher`/
  `ProviderRuntime`, API `enqueue` (Rollout/Provision targets), `smoke-staging.sh`,
  `collect-evidence.sh`, `docs/release.md`.)
- [x] Add/extend test skeletons for dispatcher selection, missing-runtime
  blocker reporting, identity plumbing, and the real-container smoke cycle;
  label compile-only work `SKELETON_READY`. (Skeletons landed directly as
  implemented tests: `dispatch::tests` unit coverage, `tests/dispatch_wiring.rs`
  9 tests, `tests/config.rs` +6 execution-field tests; no skeleton-only state.)
- [x] Confirm proposal, design, and spec agree on the Noop-only-when-blocked
  boundary and on unchanged `labrys-core` purity. (No `labrys-core` files
  changed; `git diff --stat` shows control-plane + scripts + docs only.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement startup dispatcher selection from process configuration with
  bounded, validated execution fields (runtime mode, socket path, workspace
  root, preview TTL). (`Config`: `LABRYS_RUNTIME_MODE` auto/docker/disabled,
  `LABRYS_DOCKER_BIN`, `LABRYS_WORKSPACE_ROOT`, `LABRYS_PREVIEW_TTL_SECONDS`
  60–86400; `select_dispatcher` probes and returns executable vs
  noop-blocked; `docker` mode fails startup fast when unreachable.)
- [x] Plumb `ExecutionIdentity` from claimed jobs through container,
  preview, and provider calls into persisted events, separated logs, and
  health/build-test evidence. (`identity_for_job` derives application/trace
  from the claimed job; `ExecutableDispatcher` persists the identity-attributed
  stage event + runtime log; `ProviderJobDispatcher` derives the operation
  application id from the job target; no dispatch-level evidence fabricated —
  build/test and health evidence stay with the provider runtime and preview
  manager.)
- [x] Report missing container runtime as an environment blocker at startup
  and per execution stage, never a simulated pass. (`UnavailableDispatcher`
  records the blocker with `BLOCKER_RECOVERY` per dispatch and fails the job;
  daemon startup prints `dispatcher=noop-blocked … BLOCKED … recovery: …`.)
- [x] Extend staging smoke and evidence collection with the real
  import → build → run → health → preview → cleanup cycle, keeping
  unavailable infrastructure BLOCKED with recovery. (Smoke: 17 stages incl.
  `daemon-dispatcher`, `execution-build/run-health/preview-gate/cleanup`,
  `runtime-delivery` passed 17/17 locally; evidence gains the derived
  `staging-execution` check: 14 checks, 0 failed.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise concurrent sessions, failed builds, timeouts, unhealthy
  containers, preview expiry/revocation, and redaction across the wired
  daemon; confirm no agent claim can establish health. (Existing
  `container_execution`, `provider_delivery`, `control_plane`, `api_cli`
  suites pass unmodified: `cargo test --workspace` 268/268; redaction proven
  by `executable_dispatcher_redacts_provider_failures` and
  `unavailable_dispatcher_reports_blocker_with_recovery_and_redaction`,
  which caught and fixed a dispatcher-side target leak before store
  redaction.)
- [x] Re-run core, persistence, container, provider, API/CLI, and dashboard
  suites; confirm unrelated surfaces untouched and no current-change
  placeholders remain. (Full workspace + dashboard typecheck/tests green via
  evidence; `grep` for TODO/placeholder/SKELETON in new files is clean;
  `git status` shows only change-related files.)

## 4. Verification

- [x] Run format, lint, build, unit, PostgreSQL/Docker integration,
  security, staging smoke, Gate, and strict OpenSpec validation; record
  blocked infrastructure with command, diagnostic, and next action.
  (`cargo fmt --check`, `clippy -D warnings`, `cargo build --workspace`,
  `cargo test --workspace` 268/268 vs isolated PostgreSQL 16 + Docker,
  `cargo audit` exit 0, smoke 17/17, `driftwatchdog gate` 8/8,
  `openspec validate --all --strict` pass. Pre-existing scanner self-match
  in `scripts/secret-scan.sh` fixed via character-class spelling so the
  scanner no longer flags its own pattern line; detection unchanged.)
- [x] Inspect the staged diff and confirm unrelated work is untouched.
  (Diff: control-plane `config`/`dispatch`/`providers`/bin, `config` +
  `control_plane` + new `dispatch_wiring` tests, smoke/evidence scripts,
  release docs, README counts. Untracked planning dirs for the other four
  changes left alone; pre-existing `HANDOFF.md` modification preserved for
  the pointer update.)
