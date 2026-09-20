# Labrys

Labrys is an open-source, agent-native PaaS for building, adopting, running,
and shipping applications. It connects AI Coding Agents to the boring and
dangerous parts of application delivery: runtimes, capabilities, resources,
previews, deployments, secrets, health, logs, verification, and rollback.

> If Labrys can understand how to run an application—or the application can
> be packaged as a container—Labrys should be able to host, preview, and
> operate it.

## Status

This repository is in governance and specification bootstrap. The OpenSpec
queue is a delegated implementation handoff; no runtime, build, integration,
deployment, or production capability has been implemented or verified yet.

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

The repository currently contains specifications and governance only. Select a
single active change with `openspec list`, follow `HANDOFF.md`, and implement
only the selected change plus its tests. Use the local workflow in
`.ai-rules/workflow.md`; completion rules are in `.ai-rules/completion.md`.

## Scope boundary

MVP excludes multi-agent swarms, complex workflow builders, a self-trained
foundation model, a full Kubernetes platform, GPU cloud, an app marketplace,
complete app-store automation, complex billing, long-term vector memory, and
dozens of deep framework integrations. See `ROADMAP.md` for the complete
dependency order and deferred scope.

