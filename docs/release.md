# Labrys release, rollback, and evidence procedures

## Evidence vocabulary

Every claim about the platform carries one tier. Higher tiers never follow
from lower tiers alone.

| Tier | Meaning | Produced by |
|---|---|---|
| `static` | Format, lint, specs, secret scan | CI `static` job, local Gate |
| `model-only` | Pure-core unit tests, dashboard unit/contract tests | `cargo test -p labrys-core`, `npm test` |
| `integration` | PostgreSQL-backed and Docker-backed tests, migration checks | `cargo test --workspace` with `LABRYS_DATABASE_URL`, `scripts/verify-migrations.sh` |
| `staging` | Disposable entry-point smoke (import → build → run → health → preview-gate → cleanup) | `scripts/smoke-staging.sh` |
| `production` | Runtime proof against provider scope | Manual staged runs only — never CI |

Rules:

- Unavailable infrastructure is **blocked** (command + diagnostic + next
  action), never passed.
- `failed` blocks the change and any release.
- A report with only `static`/`model-only` evidence supports **no runtime or
  production claim**; `scripts/collect-evidence.sh` writes this explicitly.

## Release procedure

1. `current_spec` must be empty (no active change) or point at the release
   change; `openspec validate --all --strict` passes.
2. `bash scripts/collect-evidence.sh --out evidence` — zero failed.
3. `bash scripts/package.sh --out dist` — inspect `release-manifest.json`
   (source revision, protocol versions, migration version, tiers) and
   `SHA256SUMS`; sign `dist/` (cosign) unless the `UNSIGNED` marker is
   resolved first.
4. `bash scripts/smoke-staging.sh --out evidence` against the packaged
   binaries.
5. Publish `dist/` + `evidence/report.json`. The manifest's
   `production_proof: false` stays until staged runtime proof exists.

## Rollback procedure

- Roll back the deployment first (`labrys rollback` with named approval;
  the database-data warning applies — data is never restored by rollback).
- Then roll back the package: redeploy the previous `dist/` by checksum.
- Migrations in this repository are forward-only; a package rollback never
  reverses applied migrations. If a migration must be undone, author a new
  compensating migration as an OpenSpec change.

## Incident procedure

1. Freeze releases (`failed` state on the release check).
2. Collect `evidence/report.json` + `labrys doctor` output for the affected
   application; keep secret references, never values.
3. Mitigate via rollback procedure above; record the rollback approval.
4. File the follow-up as an OpenSpec change with the incident evidence
   attached; archive only after Gate + strict validation pass.

## Blocked-evidence procedure

When a check reports `blocked` (e.g. no PostgreSQL, no Docker daemon, no
cosign):

1. Record the blocking command, diagnostic, and next action — do not retry
   silently and do not mark it passed.
2. Provision the missing piece (`docker compose -f compose.test.yml up -d`
   for local integration) and re-run that check.
3. If the missing piece is out of scope (production provider, signing key),
   the release ships without that tier's claim.

## Supported scope

- Runtime tiers: Tier 0 generic Dockerfile; Tier 1 language detection;
  Tier 2 framework layout. Tier 3 remains roadmap scope.
- Dispatcher selection: the daemon probes `LABRYS_DOCKER_BIN` under
  `LABRYS_RUNTIME_MODE` (`auto` default, `docker`, `disabled`) with the
  workspace root from `LABRYS_WORKSPACE_ROOT` and the preview TTL from
  `LABRYS_PREVIEW_TTL_SECONDS` (60–86400 s). A reachable runtime runs the
  executable dispatcher; otherwise the daemon reports noop-blocked with
  recovery per execution stage and never simulates a pass.
- Provider scope: local test doubles and approval-gated domain delivery.
  Cloud deployment automation is out of scope until an explicit provider
  change lands.
