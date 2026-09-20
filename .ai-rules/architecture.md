# Labrys architecture conventions

Labrys is a Rust control plane with Tokio, Axum, SQLx, Serde, Tracing, and
OpenTelemetry as the planned core stack. The dashboard is planned as Next.js,
React, TypeScript, Tailwind CSS, and shadcn/ui. Agent hosts may use Node.js or
Bun; third-party agents must be adapted through a protocol rather than
rewritten for Rust parity.

Keep these boundaries explicit:

```text
Application → Environment → Workspace/Runtime/Capabilities/Resources
Capability → Provider → Binding → Application configuration
Desired state → Controller → Actual state → Health/Events
AgentBackend → Platform API/Tools → Verifier
```

Use process-boundary protocols for ecosystem extensions; MVP uses JSON-RPC over
stdin/stdout. Generic container runtime and generic bindings are mandatory
fallbacks. Runtime profiles distinguish development from production. Repository
state is portable through optional `labrys.yaml`, while the platform database
remains authoritative for imported projects.

