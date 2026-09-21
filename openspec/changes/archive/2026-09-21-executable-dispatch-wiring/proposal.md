# Proposal: Executable dispatch wiring for the control-plane daemon

## Why

The packaged `control-plane` daemon always runs the safe `NoopDispatcher`
(`crates/labrys-control-plane/src/bin/control-plane.rs`), so API-enqueued
jobs never execute: deploys stay `InProgress`, previews never start, and
provider provisioning never runs. `scripts/smoke-staging.sh` records
runtime delivery BLOCKED and `docs/release.md` keeps `production_proof:
false`. All execution pieces exist as library code (`DockerExecutor`,
`ProviderJobDispatcher`, `PreviewManager`, `ProviderRuntime`) but nothing
wires them into the daemon process.

## What Changes

- Select the daemon dispatcher from process configuration: real
  `ProviderJobDispatcher` + `DockerExecutor` + `PreviewManager` by default,
  `NoopDispatcher` only when no container runtime is available (reported as
  an environment blocker, never a simulated pass).
- Plumb `ExecutionIdentity` (application, environment, workspace,
  correlation) from claimed jobs through container execution, preview
  lifecycle, and provider operations into persisted events, logs, and
  evidence.
- Extend staging smoke and evidence collection to exercise a real
  import → build → run → health → preview cycle against the disposable
  environment, keeping unavailable infrastructure BLOCKED with recovery.

## BFS Impact Map

- **Affected:** daemon entry point, worker dispatch, runtime execution,
  preview lifecycle, provider operations, API job finalization, staging
  smoke, evidence collection, release manifest scope.
- **Dependencies:** Phases 5–7 (persistence, container execution, provider
  adapters, API/CLI). No new provider kinds.
- **Security:** Docker socket access bounded to the daemon host, sandbox
  limits enforced before schedule, secret redaction before persistence,
  destructive provider actions still denied without named approval.
- **Unaffected:** `labrys-core` purity (no I/O added), secret cipher,
  API auth, dashboard, Tier 3 framework depth, real cloud provider
  credentials.

## Capabilities

- `executable-dispatch-wiring`

## Non-goals

- Do not add real cloud provider credentials or provisioning targets; local
  and test-double adapters remain the only provisioners.
- Do not change the secret cipher, API auth model, or dashboard surfaces.
- Do not claim production readiness; staging proof only.
