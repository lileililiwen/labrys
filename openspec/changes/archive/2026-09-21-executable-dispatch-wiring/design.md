# Design

Keep `labrys-core` I/O-free. The daemon builds its dispatcher at startup
from process configuration: when a container runtime is reachable, jobs
dispatch through `ProviderJobDispatcher` (provider operations) combined
with `DockerExecutor` (build/run/health/logs) and `PreviewManager`
(workspace materialization, health-gated URLs, expiry/revocation); when
no runtime is reachable, the daemon starts with `NoopDispatcher` and
reports every execution stage as an environment blocker with the recovery
action, never a pass.

`Config` gains explicit execution fields (runtime mode, Docker socket
path, workspace root, preview TTL) with bounded defaults and validation
so misconfiguration fails at startup before any job is claimed. Claimed
jobs carry `ExecutionIdentity` into every executor call; observations
map through `prospective_phase` and persist via the existing
`PgExecutionStore`/`PgPreviewStore`/event/log/evidence writers with
secret redaction unchanged.

Verification extends the staging smoke with a real container cycle
(import → build → run → platform health → preview URL → cleanup) against
the disposable compose environment, and the evidence report gains a
staging execution check that stays BLOCKED when Docker or PostgreSQL is
absent.
