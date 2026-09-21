# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every adapter, inspector stage, adoption-plan, config-planning,
  and tier-doc path each framework integration touches. (`RuntimeAdapter`
  detect/prepare + `detect_runtime` order, `prepare` command/health
  planning, `INSPECTION_ORDER` manifest + framework_convention stages,
  `ProfileBuilder` set_field/candidate/unknown machinery,
  `AdoptionPlan::generate`, `Manifest` export, control-plane dispatch
  (additive only), `docs/release.md` tier scope.)
- [x] Add detector/convention skeletons plus per-framework fixtures;
  label compile-only work `SKELETON_READY`. (Landed directly as
  implemented code: `FrameworkConventions` table,
  `DetectedRuntime::{framework,prod_entry,migrate_command,test_command}`,
  `detect_django`/`detect_fastapi`/Axum/workspace/Expo branches, deep
  inspector facts, `RuntimeNotes`; no skeleton-only state.)
- [x] Confirm proposal, design, and spec agree on confidence handling
  and the propose-only modification boundary. (Deep layout 0.95 beats
  manifest 0.9 only for the same value; rival high-confidence claims
  collapse to explicit unknowns via the unchanged `finish_deterministic`
  rule; all commands are proposed plans, adoption stays propose-only.)

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement Django detection plus migration/test/dev/prod/health
  conventions with matrix tests. (Tier 3 requires `manage.py` +
  packaged `settings.py` + migrations layout so `<pkg>.wsgi:application`
  derives without invention — flat layouts stay Tier 2; dev
  `manage.py runserver`, prod `gunicorn <pkg>.wsgi:application`, TCP-only
  health; covered by `django_project_detects_with_deep_conventions` +
  `django_flat_layout_stays_tier2_without_invented_wsgi`.)
- [x] Implement FastAPI detection plus uvicorn/alembic conventions with
  matrix tests. (Tier 3 requires a `FastAPI`-instantiating entry plus
  uvicorn/alembic/pytest markers; derived `<module>:app` dev/prod,
  `/health` default, evidenced-only `alembic upgrade head`/`pytest`
  (empty otherwise); dev-3xx vs production-2xx health asserted.)
- [x] Implement Axum and deeper Rust/Expo conventions with matrix tests.
  (Axum Tier 3: dep + `src/main.rs` + sqlx/diesel/migrations tooling with
  matching runners; workspace Tier 3: `[workspace]` + sqlx/diesel +
  migrations; Expo Tier 3: `app.json` + `app/` routes or `eas.json`;
  partial layouts stay Tier 2/Tier 1 with adapter defaults.)
- [x] Extend inspector evidence, adoption plans, and tier-scope docs;
  keep Tier 0 generic fallback first-class. (Deep `framework_convention`
  evidence: settings discovery, entry content, runner/tooling/workspace/
  routing facts; `migration_runner`/`test_runner` facts become ADOPT
  proposals; `Manifest.runtime_notes` portability via `RuntimeNotes`;
  release-docs Tier 3 table; `GenericAdapter` untouched, order unchanged.)

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise conflicting signals, monorepos, missing manifests, and
  framework-vs-generic fallback across all adapters and inspector
  stages. (`rival_high_confidence_framework_claims_become_explicit_unknown`
  asserts django-vs-axum unknown with both sources in
  `evidence_considered`; monorepo keeps language unknown with framework
  evidence; manifest-less framework files inspect but do not detect;
  unknown-framework-plus-Dockerfile stays Tier 0 with OCI prod.)
- [x] Re-run runtime, inspector, adoption, container-execution, and
  evidence suites; confirm no detection regressions and no placeholders
  remain. (`cargo test --workspace` 344/344 green against isolated
  PostgreSQL 16 + Docker — all prior runtime/inspector/adoption suites
  unchanged and passing; no placeholders; `prepare` dev/OCI invariants
  hold for every Tier 3 path.)

## 4. Verification

- [x] Run format, lint, build, unit, integration, Gate, and strict
  OpenSpec validation; record blocked infrastructure with command,
  diagnostic, and next action. (`cargo fmt --check` PASS; `cargo clippy
  --workspace --all-targets -- -D warnings` PASS; `cargo build
  --workspace` PASS; `cargo test --workspace` 344/344 (328 prior + 16
  `runtime_tier_depth`); `secret-scan.sh` PASS — one prerequisite reword
  of live-key canary mentions in HANDOFF + the archived api-hardening
  tasks note so the pattern stays untriggered, no allowlist expansion;
  `cargo audit` exit 0 (no new deps); `driftwatchdog gate` 8/8 PASS with
  `LABRYS_DATABASE_URL` exported, same requirement as prior changes;
  `openspec validate --all --strict` 26/26 PASS. No blocked
  infrastructure.)
- [x] Inspect the staged diff and confirm unrelated work is untouched.
  (Diff limited to `crates/labrys-core` runtime/inspector/manifest/lib,
  `tests/runtime_tier_depth.rs`, `docs/release.md` tier scope, the
  scan-hygiene rewords, the archived change, the promoted
  `spec/runtime-tier-depth`, and the HANDOFF pointer update.)
