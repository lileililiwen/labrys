# Labrys

Labrys is an open-source, agent-native PaaS for building, adopting, running,
and shipping applications. It connects AI Coding Agents to the boring and
dangerous parts of application delivery: runtimes, capabilities, resources,
previews, deployments, secrets, health, logs, verification, and rollback.

> If Labrys can understand how to run an application—or the application can
> be packaged as a container—Labrys should be able to host, preview, and
> operate it.

## Status

The OpenSpec implementation queue through Phase 7 is complete: all sixteen
changes are archived and their capability specs promoted under
`openspec/specs/`. `crates/labrys-core` holds the deterministic control-plane
model — Application and desired state, inspector and adoption, agent protocol
and policy, runtime and sandbox, workspace and preview,
capabilities/providers/bindings and sealed secrets, controllers with
idempotent jobs and resource lifecycle, OCI deployment with health-gated
promotion and explicit rollback boundaries, structured observability with an
immutable audit chain and independent verification, and the
CLI/dashboard/plugin-surface contracts. `crates/labrys-control-plane` turns
those contracts into a durable executable: PostgreSQL/SQLx persistence,
versioned migrations, a recoverable worker, bounded Docker execution,
provider/registry/domain delivery adapters, an authenticated Axum API, and
the `labrys` CLI. `dashboard/` is the Next.js operator console over that
API, and `.github/workflows/ci.yml` plus `scripts/` provide CI gates,
disposable integration environments, versioned packaging, and tiered
evidence collection (`docs/release.md`).

This is verified by 268 workspace tests (integration enabled against isolated
PostgreSQL/Docker), 19 dashboard tests, the staging smoke, and the local
Gate. It is **not** production delivery: `evidence/report.json` labels
model, integration, and staging tiers separately, the daemon runs the
executable dispatcher only when a container runtime is reachable (otherwise
it reports noop-blocked with recovery), and no production claim is made
from CI.

`openspec list` currently reports no active changes; the next step is a new
OpenSpec change or a roadmap revision.

## Product model

`Application` is the top-level object. Generated, imported, templated, forked,
and external projects share one model. Agents evolve applications; they are not
the product boundary.

The core concepts are:

- `Tool`: one action with request, execution, and result.
- `Capability`: a durable application ability such as PostgreSQL or auth.
- `Resource`: the running entity behind a capability.
- `Provider`: who supplies a capability.
- `Binding`: how a capability connects to a concrete project.
- `Runtime`: how a project is detected, built, run, exposed, and health-checked.
- `Inspector`: how an existing project is understood before changes are made.
- `AgentBackend`: who writes and maintains application code.
- `Controller`: who reconciles desired and actual state.
- `Verifier`: who independently proves an agent task is complete.

## MVP direction

The first delivery path is:

```text
Import → Inspect → Build → Run → Preview → Agent → Capability → Deploy → Observe
```

Initial runtime boundaries are Generic Container, Node, .NET, and Python, with
Rust and Expo following. Initial framework validation targets ASP.NET Core and
Next.js. Initial platform capabilities are secrets, PostgreSQL, Google Auth,
file storage, web preview, web deployment, logs, health, and rollback.

## Development

`crates/labrys-core` is a pure, deterministic Rust crate: no I/O, no clocks
beyond injected timestamps, and canonical JSON on every persisted shape.

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
node scripts/check-openspec-change-names.mjs
openspec validate --all --strict
driftwatchdog gate
```

To continue work, create an OpenSpec change (proposal, design, tasks,
scenario-based specs), set exactly one `current_spec:` pointer at the top of
`HANDOFF.md`, and implement only that change. The full lifecycle is in
`.ai-rules/workflow.md`; stopping conditions are in `.ai-rules/completion.md`;
module and boundary rules are in `.ai-rules/architecture.md`.

## Scope boundary

MVP excludes multi-agent swarms, complex workflow builders, a self-trained
foundation model, a full Kubernetes platform, GPU cloud, an app marketplace,
complete app-store automation, complex billing, long-term vector memory, and
dozens of deep framework integrations. See `ROADMAP.md` for the complete
dependency order and deferred scope.

