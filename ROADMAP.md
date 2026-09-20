# Labrys Roadmap

This roadmap is the dependency order for the delegated implementation queue.
Sequence lives here, not in numeric OpenSpec change names.

## Delivery phases

### Phase 0 — Foundation and governance

1. `application-foundation-and-desired-state`
2. `inspector-and-adoption`
3. `agent-protocol-and-policy`

Establish the shared Application model, imported-project understanding, desired
state, adoption plans, agent events, permissions, approvals, and audit shape.

### Phase 1 — Run and preview

4. `runtime-and-sandbox`
5. `workspace-and-preview`

Make container-compatible, Node, .NET, and Python applications buildable and
runnable; then add isolated worktrees, web previews, development sharing, and
the Expo mobile preview boundary.

### Phase 2 — Platform capabilities

6. `capability-provider-binding-and-secrets`
7. `controllers-and-resource-lifecycle`

Add managed, external, and adopted capabilities; providers, generic and
framework-aware bindings, encrypted secret storage, reconciliation, health,
and resource lifecycle.

### Phase 3 — Delivery and proof

8. `deployment-and-rollback`
9. `observability-and-verification`

Add OCI deployment, environments, domains, health-gated rollout, rollback,
logs, events, independent verification, and human-visible failure recovery.

### Phase 4 — Product surfaces and ecosystem

10. `cli-dashboard-and-plugin-sdk`

Expose the platform through CLI and dashboard surfaces and stabilize the
process-boundary JSON-RPC extension model for agent, inspector, runtime,
provider, binding, capability, and deployment plugins.

## Version targets

- `v0.1`: Phases 0–2, with the Run → Preview → Agent → Capability → Deploy loop
  demonstrable for the stated MVP runtimes and capabilities.
- `v0.2`: broader Rust/Expo support, Django/FastAPI/Axum integrations, stronger
  adoption, and provider/plugin expansion.
- `v1.0`: stable public protocols, self-host runtime, cloud/service separation,
  upgrade and rollback contracts, and production evidence across supported tiers.

## Runtime support tiers

- Tier 0: any project with a Dockerfile can build, run, preview, deploy, expose
  logs, and receive health checks.
- Tier 1: language/runtime version, entrypoint, ports, build, and run commands
  are detected.
- Tier 2: framework layout, migrations, tests, dev server, and conventions are
  understood.
- Tier 3: capability binding, code modification, health, migration, preview,
  and production conventions are deeply integrated.

## Deferred scope

Multi-agent swarms, complex workflow builders, a self-trained model, complete
Kubernetes, GPU cloud, an app marketplace, full app-store automation, complex
billing, long-term vector memory, and dozens of deep framework adapters remain
out of MVP scope.

