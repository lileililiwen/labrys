# Design

Provider adapters implement explicit provider traits behind controller jobs;
the core capability/provider/binding contracts remain provider-neutral. Every
operation carries application, environment, resource, correlation, and
approval context. Returned credentials are stored only through the secret
boundary and never in agent-facing events.

Registry delivery accepts only digest-addressable OCI artifacts produced by a
verified production build, verifies the pushed digest, and records immutable
deployment evidence. Domain delivery separates DNS, certificate issuance,
and traffic attachment; traffic can target only a promoted healthy deployment.
TLS and provider credentials are injected at operation time and redacted at
the adapter boundary.

Integration tests use local test providers or disposable services and cover
timeouts, retries, mismatch, adoption, approval denial, digest mismatch,
certificate failure, and safe rollback. External cloud availability is
reported separately from code/test evidence.
