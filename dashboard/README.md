# Labrys operator console

Next.js/React operator console over the versioned control-plane API
(`API_VERSION = 1`). The API and database remain authoritative; this app
holds no secret values and never reports healthy production from agent text.

## Routes

- `/` overview: platform health with agent/platform attribution kept separate.
- `/deployments`: deployment history with rollback targets (planned extension).
- `/approvals`: approval-gated mutations with named approver (planned extension).
- `/secrets`: secret references only — `ref_id`, version, scope, rotation state.

## Boundaries

- Health summarizes platform evidence with precedence
  failed > degraded > in_progress > unknown > healthy.
- Rollback always shows `DATABASE_DATA_WARNING` and requires warning
  acknowledgement, scope confirmation, target, and a named approver before
  the client calls `POST /v1/applications/{app}/rollback`.
- Secret rows reject `value`/`secret`/`plaintext`-shaped fields before render.

## Operator flows

1. Set `LABRYS_API_URL` and `LABRYS_API_TOKEN`; sign in as `human:<name>`.
2. Open overview: a platform failure banner wins over any agent success claim.
3. Open rollback: acknowledge the database-data warning, confirm scope, enter
   the named approver, then submit. The mutation posts once per idempotency key.
4. Refresh from the API after each job; `in_progress` never renders as healthy.

## Checks

- `npm install`, `npm run typecheck`, `npm test`
- `npm run build` for the production bundle inspection.

## Security notes (scoped, non-blocking)

- `cargo audit` (repo Gate) passes with exit 0.
- `npm audit` reports build/dev-time findings that are not reachable from the
  shipped static bundle: `esbuild <=0.24.2` dev-server request forgery
  (vitest/test runner only, never served), and `next`/`postcss` advisories
  covering server-function disclosure, Windows-hosted RCE, AVIF image
  optimization, and PostCSS source-map handling. The console ships static
  prerendered output with no server functions, no image optimization, no
  Windows hosting, and no operator-supplied CSS compilation. Next.js is kept
  at the latest patched 14.x (`14.2.35`); the audit-suggested remediation is
  a breaking major upgrade and is deferred until the console needs a
  server-rendered deployment.
