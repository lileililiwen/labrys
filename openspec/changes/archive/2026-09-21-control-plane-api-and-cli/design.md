# Design

The API is a thin application layer over repositories and controller jobs;
handlers validate actor scope, create idempotent commands, enqueue work, and
return structured status rather than performing provider work inline. Axum
extractors attach authenticated actor, correlation, application, and
environment context. Mutations return accepted/job references until platform
observations establish readiness.

The CLI calls the API or a local control-plane endpoint through one typed
client. Every command has stable JSON output, actionable recovery details,
exit semantics, and explicit human approval flows. Agent actors cannot invoke
human-only lifecycle mutations. Plugin and agent protocol versions are
negotiated before activation; secret fields are references or redacted.

API contract tests, CLI black-box tests, authorization tests, idempotency tests,
and an executable smoke test are required. A passing handler test does not
prove provider/runtime delivery.
