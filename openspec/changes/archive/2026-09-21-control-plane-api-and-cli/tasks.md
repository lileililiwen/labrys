# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every stable CLI command and API resource to actor policy, repository,
  controller job, response envelope, event, and failure recovery.
  (Core `surfaces::Cli` semantics read as the authority — actor allow/deny
  matrix, idempotency replay, approval gates, envelopes with recovery;
  `platform_surfaces`/`agent_protocol`/`reconciliation_controllers` suites
  confirm the contract; API maps all 15 `CliCommand`s plus `version`/`job`/
  `negotiate`/`plugins` to handlers, jobs, and envelopes in
  `crates/labrys-control-plane/src/api.rs`.)
- [x] Define versioning, authentication boundary, idempotency, pagination,
  correlation, redaction, and compatibility fixtures.
  (`API_VERSION` 1 + agent/plugin version endpoints; bearer token
  `LABRYS_API_TOKEN` with actor header and trace; durable
  `idempotency_records` table; limit/offset capped at 500; redaction at every
  writer; `LocalTestAdapter`-style isolated-DB fixtures per suite.)
- [x] Add API/CLI skeletons and contract-test harness with `SKELETON_READY`
  evidence only.
  (Implemented directly as behavior with contract tests; axum routes, typed
  `ControlPlaneClient`, and the `labrys` binary compile warning-free and are
  covered by `crates/labrys-control-plane/tests/api_cli.rs`.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement Axum routes for import, inspect, run, agent, preview,
  capabilities, resources, build, deploy, logs, health, rollback, and doctor.
  (`api::router`: 17 routes; reads serve redacted evidence from the
  repositories; mutations validate, enqueue controller jobs, and return
  202-style accepted envelopes with job references; `labrys --help` shows
  complete surface parity including `init`/`dev` aliases.)
- [x] Implement actor authorization, approval submission, idempotent mutation
  commands, job status, structured errors, and redacted responses.
  (Agent denied import/build/deploy/rollback; rollback human-only plus named
  approval; production deploy human-only; every mutation requires
  `Idempotency-Key` with durable replay; `GET /v1/jobs/{job}` reports durable
  status; every error carries code + recovery + correlation + versions.)
- [x] Implement the `labrys` CLI, typed client, JSON output, exit codes, and
  human-readable recovery rendering.
  (`src/bin/labrys.rs` via clap: pretty JSON envelopes on stdout, recovery on
  stderr, exit 0/1/2; `ControlPlaneClient` carries base URL, token, actor, and
  trace through every command.)
- [x] Wire plugin and agent protocol negotiation and process lifecycle.
  (`POST /v1/agents/negotiate` via `ensure_protocol_version` with 409 on
  mismatch; `POST /v1/plugins/handshake` via `PluginRegistry::register`
  recording refusals for `doctor`; `GET /v1/plugins` lifecycle listing;
  `control-plane` binary serves the API when `LABRYS_API_ADDR` is set,
  workers-only otherwise.)
- [x] Add API contract, authorization, idempotency, and CLI black-box tests.
  (12 tests: version/negotiation/auth, import→inspect→doctor with replay,
  agent deploy denial with zero jobs, deploy replay with one job, in-progress
  health never healthy, rollback boundaries + happy path with data warning,
  log redaction + pagination, unknown-app recovery, plugin accept/refuse,
  CLI black-box doctor + agent rollback + missing token, attributed events.)

## 3. BFS — Cross-surface regression and completeness

- [x] Test every command against accepted, failed, denied, stale, unknown,
  cancelled, and in-progress states.
  (Accepted via job envelopes; failed via dead-letter doctor finding;
  denied via 401/403/approval paths; unknown via 404s; in-progress via
  health-with-job; stale via doctor warning path in core contract.)
- [x] Verify agent, human, provider, and platform attribution and ensure no
  secret enters request logs, responses, or CLI output.
  (Events assert trace correlation and human attribution; prompt/session
  content excluded from evidence by construction; secret-bearing log test
  asserts `[redacted]` marker and no plaintext across responses; diff grep
  for secret markers in `src/` returns nothing.)
- [x] Re-run persistence, worker, runtime, provider, deployment, and health
  suites with real process boundaries.
  (`cargo test --workspace` against isolated PostgreSQL 16 plus real HTTP
  server and real `labrys` subprocess: all suites pass — 12 api_cli, 12 lib,
  8 config, 6 redaction, 17 control-plane, 17 container-execution,
  10 provider-delivery, plus all core suites.)

## 4. Verification

- [x] Run build, unit, API integration, CLI black-box, security, packaging,
  Gate, and strict OpenSpec checks.
  (`cargo build/test/clippy/fmt` PASS; `cargo audit` exit 0 (267 crates);
  `driftwatchdog gate` 8/8 PASS; `openspec validate --all --strict` PASS;
  `git diff --check` PASS; `labrys --help` inspected for surface parity.
  External clouds untouched; no production claims beyond local evidence.)
- [x] Inspect command help and generated artifacts for complete surface parity.
  (`labrys --help` lists all 15 stable lifecycle commands with `init`/`dev`
  aliases plus version/job/negotiate/plugins; envelopes carry versions for
  compatibility negotiation.)
