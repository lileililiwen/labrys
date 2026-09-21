# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map capability, binding, secret, deployment, domain, approval, rollback,
  event, and controller flows to provider operations and persistence.
  (`labrys-core` capability/controller/deployment/observability read; impact map
  in proposal `BFS Impact Map`: acceptance never flips readiness, destructive
  actions gate on approval, digest/cert evidence gates promotion/traffic.)
- [x] Define provider trait/process boundaries, credential references, retry
  policy, idempotency, redaction, and local test-provider fixtures.
  (`crates/labrys-core/src/provider.rs`: `ProviderOperation` scope +
  `idempotency_key`, secret refs only, `RetryPolicy` reuse; control plane
  `ProviderAdapter`/`OciRegistry`/`DomainDeliveryAdapter` traits with
  `LocalTestAdapter`/`LocalTestRegistry`/`LocalDomainDelivery` fixtures.)
- [x] Confirm no provider side effect is reachable without controller and
  approval policy.
  (`ProviderOperation::require_approval` runs before dispatch in
  `ProviderRuntime::execute_operation` and `ProviderJobDispatcher`; denial
  records an attributable event with zero adapter calls, covered by tests.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement capability provider adapters and readiness observation for
  PostgreSQL, auth, file storage, object storage, and generic fallback.
  (`LocalTestAdapter::{postgres,auth,file_storage,object_storage,generic}_test`
  plus `ProviderJobDispatcher` routing with generic fallback; `Accepted` keeps
  `provisioning`, only platform observation marks ready.)
- [x] Implement digest-verified OCI registry push/pull and artifact retention.
  (`gate_registry_push` + `verify_registry_digest` in core;
  `LocalTestRegistry` verifies on push, retains artifacts, records
  verified/mismatch/refused evidence in `registry_deliveries`.)
- [x] Implement DNS/TLS issuance, renewal state, domain attachment, and
  traffic routing with approval and promoted-health gates.
  (`DnsState`/`CertificateState`/`DomainDelivery` + `plan_traffic_attachment`
  in core; `LocalDomainDelivery` in control plane; cert failure keeps traffic
  detached and reports the failure, never healthy.)
- [x] Persist provider operations, credentials references, failures, usage, and
  audit events through the worker boundary.
  (Migration `20260921000002_provider_delivery.sql`: `provider_operations`
  with unique idempotency key, `registry_deliveries`, `domain_deliveries`;
  `ProviderRuntime` persists operations/evidence plus redacted events, usage,
  and hash-chained audit; `ProviderJobDispatcher` wires jobs through it.)
- [x] Add local integration adapters and failure-injection tests.
  (`InjectedFailure::{Timeout,ProviderMismatch,CredentialLeak,Transient}`;
  `crates/labrys-core/tests/provider_delivery.rs` 11 tests;
  `crates/labrys-control-plane/tests/provider_delivery.rs` 10 tests — 5 pure
  plus 5 PostgreSQL-backed against an isolated database.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise retries, duplicate requests, provider mismatch, adoption,
  approval denial, digest mismatch, certificate failure, and rollback.
  (Duplicate idempotency-key test, mismatch-at-plan test, denial tests at plan
  and runtime layers, mismatch/cert-failure tests at adapter and durable
  layers; rollback boundary asserted via `DATABASE_DATA_WARNING`.)
- [x] Verify external errors and credentials are redacted and agent completion
  never substitutes for provider readiness.
  (Redaction tests at observation, execution, and row layers with residual
  guard; `note_agent_claim_ignored` plus accepted-not-ready test; worker
  `DispatchOutcome` has no agent-claim variant.)
- [x] Re-run deployment, capability, persistence, runtime, and health suites.
  (`cargo test --workspace` against isolated PostgreSQL 16: all suites pass —
  12 lib + 8 config + 6 redaction + 17 control-plane + 17 container-execution
  + 10 provider-delivery control-plane, plus 156+11 core tests.)

## 4. Verification

- [x] Run unit, integration, security, Gate, strict OpenSpec, and artifact
  inspection checks; record unavailable external services.
  (`cargo build/test/clippy/fmt` PASS; `cargo audit` exit 0 with the scoped
  `rsa` ignore; `driftwatchdog gate` 8/8 PASS; `openspec validate --all
  --strict` PASS; `git diff --check` PASS. External clouds untouched: all
  adapters are local test doubles; no production claims beyond local evidence.)
- [x] Inspect the diff for scope and credential leakage.
  (Diff limited to provider/registry/domain contracts, adapters, migration,
  and tests; `grep` for secret markers in `src/` returns nothing; test-only
  `do-not-log` markers follow existing suite convention.)
