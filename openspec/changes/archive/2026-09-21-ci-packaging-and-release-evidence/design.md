# Design

CI runs dependency-ordered gates with isolated PostgreSQL and container
services, caches only immutable dependencies, and uploads test, migration,
security, artifact, and runtime evidence. The local `.ai-gate/gate.yaml`
remains policy; shared gate runtimes stay external.

Packaging uses reproducible version inputs and emits checksums, SBOM metadata,
container/image digests, migration version, protocol versions, and supported
runtime/provider tiers. Release smoke tests exercise import → inspect → build
→ run → preview → capability → deploy → observe with a disposable environment.
The evidence report labels model-only, integration, staging, and production
verification separately.
