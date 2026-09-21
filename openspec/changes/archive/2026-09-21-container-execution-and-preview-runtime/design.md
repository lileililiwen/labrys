# Design

Keep `labrys-core` pure and place Docker/OCI-process I/O in a control-plane
runtime crate. Use an injected executor trait with a Docker implementation,
structured command arguments, cancellation tokens, and explicit resource
limits. All executions receive an application, environment, workspace, and
correlation identity. Build output becomes a verified artifact reference; a
development process never becomes a production image implicitly.

Preview controllers create per-session workspaces, start the selected runtime,
wait for platform-owned health, then issue temporary access metadata. Failed,
expired, or revoked previews close processes and revoke access. Logs stream
through the separated log store and pass redaction before persistence.

Verification must include container integration tests for a generic Dockerfile,
timeout, memory/process limits, network posture, health failure, cleanup, and
concurrent sessions. Runtime availability must be reported as an environment
blocker when Docker is absent, never simulated as a pass.
