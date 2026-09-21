# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every provider-adapter, registry, domain-delivery, runtime
  persistence, dispatch, secrets, approval, and evidence path the real
  adapters touch. (`ProviderAdapter`/`OciRegistry`/`DomainDeliveryAdapter`
  traits, `LocalTest*` doubles, `ProviderRuntime` persistence boundary,
  `ProviderJobDispatcher` key routing, `ExecutableDispatcher::build`,
  `Config` process fields, `converge_domain`/`push_artifact` evidence rows,
  `docs/release.md` supported scope.)
- [x] Add adapter/test-double skeletons and disposable-environment fixtures;
  label compile-only work `SKELETON_READY`. (Skeletons landed directly as
  implemented code: `postgres_provider`, `storage_provider`, `oci_client`,
  `tls_dns` modules plus `tests/real_provider_delivery.rs` with an isolated
  `labrys_real_provider_test` database; no skeleton-only state.)
- [x] Confirm proposal, design, and spec agree on credential sourcing,
  approval gates, and unchanged trait shapes. (Credentials from
  `LABRYS_PROVIDER_*` process config plus process-memory caches only; the
  destructive gate stays in `ProviderRuntime`; traits extended additively
  with defaults — `push_content`, async `certificate_evidence` — and
  `labrys-core` is untouched.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement the managed PostgreSQL and object-storage provisioners with
  least-privilege credentials from secret references.
  (`PostgresProvisioner`: least-privilege roles/databases/grants, catalog
  readiness, approval-gated drop, idempotent re-provision; `FsBucketBackend`
  with 0700 buckets, scoped keys, revocation; Ambos covered by
  `postgres_provision_is_least_privilege_and_redacted`,
  `storage_buckets_provision_with_scoped_keys_and_revoke`,
  `storage_readiness_after_cache_loss_reports_recovery`. Role proved unable
  to create databases/roles or read foreign tables; no credential value in
  any persisted row.)
- [x] Implement OCI registry push with digest fetch and mismatch refusal.
  (`OciRegistryClient`: real Distribution blob/manifest round-trip,
  `Docker-Content-Digest` verification against computed and recorded
  digests; `push_content` runtime method persisting verified/mismatch
  rows; `registry_push_round_trip_verifies_digest` against a real
  `registry:2` container; unverified and contentless pushes refused.)
- [x] Implement DNS/TLS issuance plus approval-gated traffic attachment
  that never routes to unpromoted deployments. (`ResolvingDns` via the
  system resolver, `OpensslTlsIssuer` via structured `openssl` argv with
  0600 keys, `RealDomainDelivery` over unchanged
  `plan_traffic_attachment`; `tls_issuance_persists_fingerprint_not_keys`
  attaches on localhost with fingerprint evidence, `cert_failure_*`
  reports the blocker with zero traffic.)
- [x] Persist operations, observations, and digest/certificate evidence
  through `ProviderRuntime` with redaction, and update supported-scope docs.
  (No new persistence paths: `execute_operation`, `push_content`,
  `converge_domain` with async certificate evidence; `docs/release.md`
  names the executable surface and its explicit out-of-scope bounds.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise credential-bearing failures, approval denials with zero
  provider calls, digest mismatches, cert failures, and concurrent
  operations across all adapters. (`postgres_delete_without_approval_*`
  denies with the database intact, mismatch persisted without promotion,
  `concurrent_operations_across_adapters_stay_isolated` provisions and
  tears down postgres + 2 buckets with distinct keys, daemon routing proven
  by `daemon_build_registers_real_storage_from_config`.)
- [x] Re-run worker, API/CLI, dashboard, and evidence suites; confirm no
  credential material in events/logs/evidence and no placeholders remain.
  (`cargo test --workspace` 299/299; row-level redaction assertions in the
  postgres/storage/TLS tests; placeholder scan clean; unrelated suites
  untouched.)

## 4. Verification

- [x] Run format, lint, build, unit, PostgreSQL-backed provider
  integration, security, Gate, and strict OpenSpec validation; record
  blocked external infrastructure with command, diagnostic, and next
  action. (`cargo fmt --check`, `clippy -D warnings`, `cargo build`,
  299/299 vs isolated PostgreSQL 16 + Docker + openssl, `cargo audit`
  exit 0 with new `sha2` dep, smoke 17/17, evidence 14 checks 0 failed,
  `driftwatchdog gate` 8/8, `openspec validate --all --strict` pass.
  Docker/registry/openssl absence skips with diagnostics, never passes.)
- [x] Inspect the staged diff and confirm unrelated work is untouched.
  (Diff: 4 new modules, provider trait defaults, `ProviderRuntime`
  content/evidence methods, dispatch wiring, `Config` provider fields,
  tests, `Cargo.toml`/`Cargo.lock` for `sha2`, release docs, README
  counts. Other three planning dirs left untracked; `HANDOFF.md` reserved
  for the pointer update.)
