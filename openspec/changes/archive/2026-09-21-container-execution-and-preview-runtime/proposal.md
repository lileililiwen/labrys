# Proposal: Real container execution and preview runtime

## Why

`labrys-core` only simulates builds, runs, and health checks. No Docker/container
executor, workspace materialization, preview process, log stream, or runtime
integration exists, so the advertised generic-container minimum is not
executable.

## What Changes

- Add a bounded container executor for build, run, health, logs, limits, and
  cleanup using the existing runtime and sandbox contracts.
- Materialize isolated session workspaces and health-gated web/mobile previews.
- Persist execution and preview state through the persistence/worker change.
- Add integration tests against an isolated container runtime.

## BFS Impact Map

- **Affected:** runtime adapters, sandbox limits, workspaces, previews, logs,
  controller jobs, health and verification evidence.
- **Dependencies:** `control-plane-persistence-and-worker`.
- **Security:** no host mounts beyond an approved workspace, read-only root by
  default, isolated network, dropped capabilities, bounded CPU/memory/PIDs,
  timeout cancellation, and redacted logs.
- **Unaffected:** registry push, TLS, public API, dashboard, and provider
  provisioning.

## Capabilities

- `container-runtime-execution`
- `preview-execution`

## Non-goals

- Do not implement Kubernetes, GPU scheduling, or arbitrary privileged Docker.
- Do not treat simulated core functions as runtime evidence.
