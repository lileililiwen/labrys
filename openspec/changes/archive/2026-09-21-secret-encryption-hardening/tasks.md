# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every seal/open, rotation, redaction, persistence-writer, API
  response, dashboard-render, and scan path that handles secret material.
  (`seal`/`open`/`keystream`/`rotate` in `crates/labrys-core/src/capability.rs`;
  `redact.rs` reject-before-SQL guards plus `redact_guard` on every
  control-plane writer in `observability.rs`/`providers.rs`/`api.rs`/`worker.rs`;
  typed API envelopes and CLI pretty-JSON (references only); dashboard
  reference-only rendering in `dashboard/src/lib/secrets.ts`; `secret-scan.sh`
  and `redaction.rs` suites. No new writer surface: control-plane already
  rejects plaintext before SQL.)
- [x] Add cipher-envelope skeletons and known-plaintext fixtures; label
  compile-only work `SKELETON_READY`. (Landed directly as implemented code:
  `SECRET_ENVELOPE_VERSION = 1` ChaCha20-Poly1305 envelopes with per-seal
  random nonces plus `SECRET_ENVELOPE_LEGACY_XOR = 0` read path; known
  plaintexts `s3cr3t-db-value`/`test-api-key-9f2e` swept through event, log,
  evidence, API, and dashboard payloads; no skeleton-only state.)
- [x] Confirm proposal, design, and spec agree on envelope versioning and
  the values-never-in-audit invariant. (Readers dispatch on `cipher_version`;
  pre-rotation envelopes stay readable until rotated; `SecretAudit` carries
  reference id, version lineage, actor, approver only — values excluded by
  construction, asserted by sweep tests.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement AEAD seal/open with versioned envelopes and external-key
  loading (zeroized, never serialized). (`MasterKey` holds raw material with
  `Drop` zeroization and redacted `Debug`; AEAD key is `SHA256(material)`;
  `from_env`/`from_file`/`generate` loaders naming only var/path in errors;
  `seal` binds ref id + secret version as AAD; `open` dispatches v1 AEAD /
  v0 legacy XOR / unknown-version refusal; tamper and wrong-key opens fail
  closed. Covered by in-module `secret_cipher_tests` (4 tests) and
  `seal_and_reopen_round_trips_and_tamper_is_a_verification_failure`.)
- [x] Implement rotation with approval gating for production scopes and
  value-free audit lineage. (`rotate` now takes `&ExplicitApproval`, denied
  before any re-seal for production scopes with recovery, fresh nonce per
  re-seal, `approved_by` recorded; new `rotate_key` re-seals all live
  references under a new key atomically after opening all under the old key,
  killing the old key. Covered by `production_rotation_without_approval_*`,
  `key_rotation_reseals_everything_and_kills_the_old_key`,
  `non_production_key_rotation_needs_no_approval`.)
- [x] Harden all writers and response boundaries to reject secret values
  before persistence/serialization, failing closed. (New core
  `SecretStore::guard_text` fail-closed boundary plus existing
  control-plane `redact_guard` before-SQL writers, reference-only API/CLI
  envelopes, and reference-only dashboard rendering — all re-verified by
  `guards_keep_secret_values_out_of_every_payload_boundary` and the
  unchanged redaction/API/CLI suites.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise cross-version reads, rotation under concurrency, tampered
  envelopes, and known-plaintext sweeps across events, logs, evidence,
  API, and dashboard payloads. (`legacy_v0_envelope_opens_until_rotated`,
  `old_envelope_after_rotation_remains_readable_until_rotated`,
  `rotation_under_concurrency_*` via scoped threads + Mutex asserting
  version 5 and intact value, AAD ref/version binding, nonce uniqueness,
  ciphertext-at-rest carries no plaintext window.)
- [x] Re-run capability, persistence, API/CLI, dashboard, and scan suites;
  confirm no plaintext in any persisted or rendered artifact and no
  placeholders remain. (`cargo test --workspace` 312/312 green against
  isolated PostgreSQL 16 + Docker; `secret-scan.sh` PASS; no placeholders;
  only caller churn is the `rotate` approval parameter in
  `capability_secrets.rs`.)

## 4. Verification

- [x] Run format, lint, build, unit, integration, secret-scan, Gate, and
  strict OpenSpec validation; record blocked infrastructure with command,
  diagnostic, and next action. (`cargo fmt --check` PASS; `cargo clippy
  --workspace --all-targets -- -D warnings` PASS; `cargo build --workspace`
  PASS; `cargo test --workspace` 312/312; `cargo audit` exit 0 — 4 new
  deps `chacha20poly1305`/`rand`/`sha2`/`zeroize` clean; `secret-scan.sh`
  PASS; `driftwatchdog gate` 8/8 PASS with `LABRYS_DATABASE_URL` pointing
  at the isolated PG (gate's `cargo test --workspace` needs a database
  for infra-gated suites, same as prior changes); `openspec validate
  --all --strict` 26/26 PASS. No blocked infrastructure: the one
  `real_provider_delivery` failure seen without the DB env was the
  documented harness requirement, resolved by exporting the isolated URL.)
- [x] Inspect the staged diff and confirm unrelated work is untouched.
  (Diff limited to `crates/labrys-core/{Cargo.toml,Cargo.lock via workspace
  lock,src/capability.rs,src/lib.rs,tests/capability_secrets.rs}`,
  new `tests/secret_hardening.rs`, the archived change, the promoted
  `spec/secret-encryption-hardening`, and the HANDOFF pointer update.)
