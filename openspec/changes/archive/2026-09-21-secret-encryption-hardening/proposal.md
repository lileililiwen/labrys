# Proposal: Secret encryption hardening

## Why

`SecretStore` seals values with a deterministic XOR keystream
(`crates/labrys-core/src/capability.rs`, `seal`/`keystream`) documented
as a stand-in for encrypted PostgreSQL. A committed-database reader can
recover plaintext structure, there is no key-rotation path that keeps
history auditable, and no boundary proves secrets never enter LLM or
event context beyond the current redaction helpers.

## What Changes

- Replace the XOR stand-in with an AEAD seal (nonce-misuse-resistant,
  per-reference nonces) keyed by an external `MasterKey` loaded from
  process configuration, with versioned envelopes so old seals stay
  readable until rotated.
- Add explicit rotation (`rotate`) that re-seals live references, keeps
  neither old nor new values in the audit record, and requires human
  approval for production scopes (existing boundary, now enforced on a
  real cipher).
- Add startup and periodic guards asserting no secret value reaches
  events, logs, evidence, API responses, or dashboard payloads, failing
  closed with redaction plus rejection.

## BFS Impact Map

- **Affected:** `SecretStore` seal/open, `MasterKey` loading, secret
  rotation, redaction helpers, control-plane persistence writers, API/CLI
  safe responses, dashboard reference-only rendering, secret-scan and
  redaction tests.
- **Dependencies:** none (parallelizable with dispatch wiring); real
  provider adapters consume the hardened store but do not block it.
- **Security:** key from environment/KMS-style handle only, never stored
  beside ciphertext; AEAD authentication failures surface as
  verification failures with recovery, never plaintext; rotation is
  auditable without exposing values.
- **Unaffected:** capability/binding resolution, provider selection,
  deployment/rollback semantics, API auth, preview lifecycle.

## Capabilities

- `secret-encryption-hardening`

## Non-goals

- Do not build a full KMS/Vault integration; the key arrives as an
  external handle plus local file/env loading with clear rotation docs.
- Do not change the secret-reference envelope shape consumed by bindings.
- Do not weaken the production-approval boundary for rotation/replace.
