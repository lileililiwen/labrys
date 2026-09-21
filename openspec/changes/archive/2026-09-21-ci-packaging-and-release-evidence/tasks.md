# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every crate/app/service artifact and required check to CI jobs,
  fixtures, caches, evidence outputs, and release gates.
- [x] Define reproducible versioning, migration compatibility, supported tiers,
  security policy, artifact provenance, and evidence vocabulary.
- [x] Add CI/package skeletons and a disposable integration environment with
  `SKELETON_READY` evidence.

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement Rust, SQLx/PostgreSQL, container, API/CLI, dashboard, OpenSpec,
  formatting, lint, test, audit, and secret-scan workflows.
- [x] Implement versioned binaries/packages, checksums, SBOM, image metadata,
  migration verification, and plugin protocol compatibility checks.
- [x] Implement end-to-end staging smoke tests and evidence collection for
  supported generic/Native runtime and provider tiers.
- [x] Document release, rollback, incident, and blocked-evidence procedures.

## 3. BFS — Cross-surface regression and completeness

- [x] Run the full lifecycle in a disposable environment and verify cleanup,
  rollback, logs, health, approvals, and redaction.
- [x] Verify CI failures block release and that model-only success cannot create
  runtime or production claims.
- [x] Re-run all package checks and inspect artifacts independently of CI logs.

## 4. Verification

- [x] Run local gate, CI-equivalent checks, package inspection, migration,
  security, smoke, and strict OpenSpec checks.
- [x] Record unavailable external services as blocked evidence with next action.

Evidence: `.github/workflows/ci.yml` (static, rust+postgres service,
dashboard-build, security, package-and-evidence jobs),
`compose.test.yml` (disposable postgres:16), `scripts/secret-scan.sh`,
`scripts/verify-migrations.sh` (6/6 applied, schema 20260922000001),
`scripts/package.sh` (release binaries, dashboard bundle, SHA256SUMS,
release-manifest.json, lockfile SBOM, cosign-or-UNSIGNED),
`scripts/smoke-staging.sh` (10/10 stages pass; runtime-delivery honestly
blocked on the no-op dispatcher), `scripts/collect-evidence.sh` (13 checks,
0 failed; production-proof always blocked), `docs/release.md` (evidence
vocabulary, release/rollback/incident/blocked procedures),
`.secret-scan-allowlist` (fake redaction-test fixtures only).
`cargo test --workspace` 249/249 with integration enabled;
`driftwatchdog gate` 8/8; `openspec validate --all --strict` passes.
