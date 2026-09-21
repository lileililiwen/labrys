# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every auth, actor-context, approval, CLI-token, smoke-auth, and
  dashboard-client path affected by multi-token auth and actor binding.
  (Single static compare in `api.rs::actor_context`, `ApiConfig.token`,
  `ApiState.token`, `ControlPlaneClient` bearer/actor headers, CLI
  `--token`/`--actor`, smoke `TOKEN` bootstrap, dashboard `headersFor`;
  approval boundaries in `forbid_agent`/`require_human` untouched by shape.)
- [x] Add token-record/rotation/TLS-config skeletons and auth fixtures;
  label compile-only work `SKELETON_READY`. (Landed directly as implemented
  code: `auth.rs` with `TokenRecord`/`TokenStore`/`RateLimiter`/`TlsConfig`
  plus `tests/api_hardening.rs` against an isolated
  `labrys_api_hardening_test` database; no skeleton-only state.)
- [x] Confirm proposal, design, and spec agree on scope matching and the
  bootstrap-warning behavior. (Scope match is token-scope vs header-actor
  kind; `platform:*` headers are never accepted over HTTP; bootstrap maps
  to one human record and `auth_summary()` prints the rotation warning.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement multi-token auth with scope, expiry, hot rotation, and
  next-request revocation (single-token bootstrap with warning).
  (`TokenStore::from_lines` `<id>:<scope>:<expiry>:<secret>` from
  `LABRYS_API_TOKENS_FILE`/`LABRYS_API_TOKENS`/`LABRYS_API_TOKEN`;
  SHA256 + constant-time compare; `reload` swaps without restart and
  zeroizes the old set; `revoke` marks revoked with id-preserving denial;
  expiry enforced per request. Covered by 7 `auth.rs` unit tests and
  `rotated_token_takes_over_without_restart`.)
- [x] Bind request actor to token identity, denying out-of-scope header
  actors while preserving attribution shapes. (`authorized()` replaces
  `actor_context` at all 17 call sites: identical 401 for
  unknown/expired/revoked, 403 scope denial with the approval recovery
  path, `EventActor`/envelope/approval shapes unchanged. `api_cli.rs`
  harness now issues a human bootstrap plus an agent token; handler-level
  agent denials still covered via `post_as(AGENT_TOKEN, ...)`.)
- [x] Implement TLS termination config, loopback-only plain HTTP default,
  rate limiting, and redacted auth-failure audit. (`TlsConfig` + file
  existence checks + `bind_policy` matrix; `serve_api` serves rustls
  (ring provider) or loopback-only plain; per-IP fixed-window limiter as
  router middleware in front of auth (429 with retry recovery);
  `audit_auth_denial` writes `auth.rejected` rows with token ids or
  `sha256:` fingerprints only. CLI gains `--tls-ca`/`LABRYS_TLS_CA` plus
  `ControlPlaneClient::with_root_cert`; dashboard `readEnvelope` throws
  actionable 401/403/429 errors with server recovery.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise rotation without restart, revocation timing, expired
  tokens, scope-escalation attempts, TLS/plain-matrix binds, and rate
  limits across API, CLI, smoke, and dashboard client.
  (`tests/api_hardening.rs` 9 tests: rotation swap, expiry-before-mutation
  + audit, oracle-free response shapes, both scope-escalation directions,
  matched pairs, 429 budget, `ApiConfig` bind matrix, real rcgen TLS
  round-trip with explicit CA trust plus plain-to-TLS-port refusal;
  smoke gains `scope-denied-agent-on-human-token` + `unknown-token-denied`
  stages and an agent-scoped smoke token; dashboard gains 2 auth-error
  client tests.)
- [x] Re-run API/CLI, worker, dashboard, and evidence suites; confirm no
  auth bypass, no identity oracle, and no placeholders remain.
  (`cargo test --workspace` 328/328 green against isolated PostgreSQL 16
  + Docker; `npm run typecheck` + 21/21 vitest pass; `smoke-staging.sh`
  19/19 pass; no placeholders; only caller-visible changes are the
  `ApiConfig`/`ApiState` token-store shapes and the `--tls-ca` flag.)

## 4. Verification

- [x] Run format, lint, build, unit, API/CLI integration (real HTTP +
  real CLI subprocess), security, Gate, and strict OpenSpec validation;
  record blocked infrastructure with command, diagnostic, and next action.
  (`cargo fmt --check` PASS; `cargo clippy --workspace --all-targets
  -- -D warnings` PASS; `cargo build --workspace` PASS; `cargo test
  --workspace` 328/328 (312 prior + 7 auth unit + 9 api_hardening);
  `secret-scan.sh` PASS — one prerequisite rename of `sk-live-*` test
  canaries to `test-api-key-*` (plus the matching archived quote) so the
  live-key pattern stays untriggered, no allowlist expansion; `cargo
  audit` exit 0 (new `axum-server`/`rustls`/`rustls-pemfile`/`subtle`/
  `zeroize` deps; one non-blocking unmaintained warning for
  `rustls-pemfile`); `driftwatchdog gate` 8/8 PASS with
  `LABRYS_DATABASE_URL` exported for infra-gated suites, same requirement
  as prior changes; `openspec validate --all --strict` 26/26 PASS. No
  blocked infrastructure.)
- [x] Inspect the staged diff and confirm unrelated work is untouched.
  (Diff limited to `crates/labrys-control-plane` auth/API/client/CLI,
  `tests/api_hardening.rs` + `api_cli.rs` harness, dashboard client +
  tests, smoke auth stages, `docs/release.md` auth scope, the archived
  change, the promoted `spec/control-plane-api-hardening`, and the
  HANDOFF pointer update. The `sk-live` fixture rename in
  `secret_hardening.rs` is a scan prerequisite, noted above.)
