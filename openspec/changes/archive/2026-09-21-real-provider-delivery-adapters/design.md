# Design

New adapters implement the existing traits without changing
`labrys-core` contracts: a PostgreSQL provisioner (server/role/database
creation with least-privilege credentials), an object-storage provisioner
(bucket + scoped keys), an OCI registry client (push, digest fetch,
mismatch refusal), and a DNS/TLS adapter (record + certificate issuance,
approval-gated traffic attachment via `plan_traffic_attachment`).

Each adapter receives only secret references plus scoped, short-lived
tokens loaded from process configuration at daemon startup; the runtime
resolves values at call time and `sanitize_failure` redacts them from
every persisted observation. `ProviderRuntime.execute_operation` stays
the single persistence boundary, recording operation rows, observations,
and digest/certificate evidence. Unknown provider keys keep the current
retryable-failure-with-recovery behavior.

Verification adds disposable-Espanha integration tests per adapter
(provision → observe → delete with approval, digest round-trip, cert
failure without traffic) and updates the release supported-scope docs to
name the newly executable provider surface.
