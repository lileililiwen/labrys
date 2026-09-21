# Labrys Roadmap

This roadmap is the dependency order for the delegated implementation queue.
Sequence lives here, not in numeric OpenSpec change names.

All twenty-one changes below are implemented, locally verified, and archived under
`openspec/changes/archive/`; their capability specs are promoted to
`openspec/specs/`. The queue is exhausted — extend this roadmap (or open a new
change) before resuming implementation.

## Delivery phases

### Phase 0 — Foundation and governance — ARCHIVED

1. `application-foundation-and-desired-state`
2. `inspector-and-adoption`
3. `agent-protocol-and-policy`

Establish the shared Application model, imported-project understanding, desired
state, adoption plans, agent events, permissions, approvals, and audit shape.

### Phase 1 — Run and preview — ARCHIVED

4. `runtime-and-sandbox`
5. `workspace-and-preview`

Make container-compatible, Node, .NET, and Python applications buildable and
runnable; then add isolated worktrees, web previews, development sharing, and
the Expo mobile preview boundary.

### Phase 2 — Platform capabilities — ARCHIVED

6. `capability-provider-binding-and-secrets`
7. `controllers-and-resource-lifecycle`

Add managed, external, and adopted capabilities; providers, generic and
framework-aware bindings, encrypted secret storage, reconciliation, health,
and resource lifecycle.

### Phase 3 — Delivery and proof — ARCHIVED

8. `deployment-and-rollback`
9. `observability-and-verification`

Add OCI deployment, environments, domains, health-gated rollout, rollback,
logs, events, independent verification, and human-visible failure recovery.

### Phase 4 — Product surfaces and ecosystem — ARCHIVED

10. `cli-dashboard-and-plugin-sdk`

Expose the platform through CLI and dashboard surfaces and stabilize the
process-boundary JSON-RPC extension model for agent, inspector, runtime,
provider, binding, capability, and deployment plugins.

### Phase 5 — Control-plane execution foundation — ARCHIVED

11. `control-plane-persistence-and-worker`

Replace in-memory-only runtime seams with durable PostgreSQL/SQLx state,
transactional evidence persistence, and a recoverable controller worker. This
is the prerequisite for executable API/CLI surfaces and real provider/runtime
adapters.

12. `container-execution-and-preview-runtime`

Turn runtime and preview contracts into bounded Docker execution, isolated
workspaces, health-gated previews, logs, and cleanup.

13. `provider-registry-and-domain-adapters`

Execute capability provisioning, OCI registry delivery, DNS/TLS issuance, and
approval-gated domain traffic through provider adapters.

### Phase 6 — Product entry points — ARCHIVED

14. `control-plane-api-and-cli`

Expose the persisted control plane through an authenticated Axum API and the
stable `labrys` CLI with jobs, approvals, attribution, and safe responses.

15. `dashboard-and-operator-console`

Deliver the operator web console over the API with health, evidence,
previews, approvals, logs, deployments, and accessible failure recovery.

### Phase 7 — Release and runtime proof — ARCHIVED

16. `ci-packaging-and-release-evidence`

Add CI, packaging, migration checks, security gates, reproducible artifacts,
and staged lifecycle evidence. This package does not convert local model tests
into production claims without runtime proof.

### Phase 8 — Execution hardening and runtime depth — ARCHIVED

17. `executable-dispatch-wiring`
18. `real-provider-delivery-adapters`
19. `secret-encryption-hardening`
20. `control-plane-api-hardening`
21. `runtime-tier-depth-expansion`

Wire the executable dispatcher with identity plumbing and staging execution
proof; deliver real provider adapters (managed PostgreSQL, filesystem
storage, OCI registry, DNS/TLS); seal secrets with a real AEAD cipher and
approval-gated rotation; harden API auth (scoped multi-token credentials,
actor binding, TLS termination, rate limits, redacted auth audit); and add
Tier 3 framework depth (Django, FastAPI, Axum, Rust workspaces, Expo) with
layout-derived conventions that are proposed, never auto-run.

## Version targets

- `v0.1`: Phases 0–2, with the Run → Preview → Agent → Capability → Deploy loop
  demonstrable for the stated MVP runtimes and capabilities. Contracts and
  deterministic models are in place; the demonstrable loop still needs the
  SQLx persistence, container execution, and CLI binaries built on them.
- `v0.2`: delivered in Phase 8 — broader Rust/Expo support,
  Django/FastAPI/Axum integrations, stronger adoption (proposed migration
  and test runners), and provider/plugin expansion. Remaining: further
  framework adapters beyond the Tier 3 table in `docs/release.md`.
- `v1.0`: stable public protocols, self-host runtime, cloud/service separation,
  upgrade and rollback contracts, and production evidence across supported tiers.

## Runtime support tiers

- Tier 0: any project with a Dockerfile can build, run, preview, deploy, expose
  logs, and receive health checks.
- Tier 1: language/runtime version, entrypoint, ports, build, and run commands
  are detected.
- Tier 2: framework layout is detected with adapter-level dev/prod commands;
  partial layouts fall back here without invented entries.
- Tier 3: deep framework integrations (Django, FastAPI, Axum, Rust
  workspaces, Expo — see the per-framework table in `docs/release.md`) with
  layout-derived migration, test, dev-server, production, and health
  conventions. Commands are proposed plans surfaced through adoption items
  and `labrys.yaml` runtime notes; they never auto-run without a reviewed
  job under approval.

## Deferred scope

Multi-agent swarms, complex workflow builders, a self-trained model, complete
Kubernetes, GPU cloud, an app marketplace, full app-store automation, complex
billing, long-term vector memory, and dozens of deep framework adapters remain
out of MVP scope.
