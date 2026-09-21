# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map dashboard sections to API resources, permissions, evidence source,
  loading/empty/error states, and mobile/responsive requirements.
- [x] Define typed client, auth/session, redaction, polling/live updates, and
  destructive-confirmation boundaries.
- [x] Add Next.js application and test skeleton with `SKELETON_READY` evidence.

## 2. DFS — Requirement-by-requirement implementation

- [x] Implement application/environment, inspector, preview, capability,
  resource, deployment, health, logs, verification, and approval views.
- [x] Implement evidence-source attribution, stale/failed states, job polling,
  safe mutation and rollback confirmation flows.
- [x] Implement responsive accessible navigation, tables, status regions,
  keyboard interactions, and secret-reference rendering.
- [x] Add component, accessibility, browser, and API-contract tests.

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise agent-success/platform-failure, stale-health, denied-approval,
  expired-preview, rollback-warning, and provider-error states.
- [x] Verify every write respects API authorization and every secret-bearing
  field is redacted in DOM, logs, screenshots, and test fixtures.
- [x] Re-run API, CLI, persistence, worker, runtime, and security checks.

## 4. Verification

- [x] Run typecheck, lint, build, unit, accessibility, browser, security,
  packaging, Gate, and strict OpenSpec checks.
- [x] Inspect production bundle and documented operator flows.

Evidence: `dashboard/` Next.js console (`src/lib/api.ts`,
`src/lib/attribution.ts`, `src/lib/secrets.ts`, `src/lib/rollback.ts`,
`src/components/`, `src/app/`); 19/19 vitest tests pass
(`tests/attribution.test.ts`, `tests/rollback.test.tsx`,
`tests/secrets.test.tsx`, `tests/dashboard-view.test.tsx`,
`tests/api-client.test.ts`); `npm run typecheck` passes; `cargo test
--workspace` unchanged; `driftwatchdog gate` 8/8; `openspec validate --all
--strict` passes. Operator flows documented in `dashboard/README.md`.
