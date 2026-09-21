# Proposal: Executable control-plane API and CLI

## Why

The repository exposes only Rust library contracts. The promised lifecycle CLI
and a process boundary for agents, plugins, and operators do not exist, so no
user or worker can invoke the platform through a stable interface.

## What Changes

- Add an Axum control-plane API over persisted application, preview,
  capability, deployment, health, logs, verification, and approval operations.
- Add a distributable Rust `labrys` CLI using machine-readable response
  envelopes and stable command semantics from `platform-surfaces`.
- Add authentication/actor mapping, correlation, idempotency, pagination,
  error recovery, and protocol/version negotiation.
- Expose the existing JSON-RPC plugin and agent boundaries through the API and
  CLI without putting secrets in requests or responses.

## BFS Impact Map

- **Dependencies:** persistence/worker, container execution, provider/domain
  adapters.
- **Affected:** all mutating controllers, approvals, events, health, logs,
  plugin processes, and operator workflows.
- **Unaffected:** dashboard visual design and cloud-specific providers.

## Capabilities

- `control-plane-api`
- `labrys-cli`

## Non-goals

- Do not build the dashboard in this change.
- Do not expose raw shell or secret values through public endpoints.
