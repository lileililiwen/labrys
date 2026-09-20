# Tasks

## 1. BFS — Baseline and impact coverage

- [x] Map every persisted aggregate and repository method to its owner table,
  transaction boundary, redaction rule, and canonical spec requirement.
- [x] Map `JobQueue`, all controller dispatch paths, event/audit/evidence
  writers, and restart/lease behavior to integration fixtures and failure
  scenarios.
- [x] Add the control-plane crate/configuration skeleton, migration layout,
  PostgreSQL test harness, and explicit `SKELETON_READY` evidence without
  claiming runtime behavior.
- [x] Confirm proposal, design, canonical specs, schema constraints, and
  deferred boundaries agree before implementing adapters.

## 2. DFS — Requirement-by-requirement implementation

- [x] Add versioned PostgreSQL migrations for applications, environments,
  desired/observed state, capabilities/resources, deployments, events,
  audit/evidence, usage, and jobs with ownership and uniqueness constraints.
- [x] Implement SQLx application/state repositories with transaction-scoped
  writes, optimistic generation checks, strict environment isolation, and
  round-trip mapping to `labrys-core` contracts.
- [x] Implement redacting persistence adapters for events, logs, audit,
  verification evidence, and usage; reject plaintext secret values before
  insert and preserve safe recovery details.
- [x] Implement durable job claim, completion, retry/backoff, pause/resume,
  dead-letter, requeue, idempotency, lease expiry, and graceful shutdown.
- [x] Wire the worker to controller dispatch and platform-owned observations;
  prove agent completion cannot promote readiness.
- [x] Add process configuration, migration startup, structured worker
  attribution, and integration fixtures using an isolated PostgreSQL service.

## 3. BFS — Cross-surface regression and completeness

- [x] Exercise concurrent workers, process restart, expired leases, duplicate
  requests, transaction rollback, generation conflicts, and partial provider
  failure across all affected repositories and controllers.
- [x] Verify desired state, observed state, event/audit history, health,
  verification, usage, and job state remain consistent after retry and
  recovery; include secret-redaction assertions on persisted rows and errors.
- [x] Re-run the existing pure-core suite and inspect all callers for hidden
  in-memory assumptions, migration gaps, unsafe logging, or unbounded retries.
- [x] Remove current-change placeholders and record the exact unavailable
  external infrastructure evidence instead of treating local integration
  success as production readiness.

## 4. Verification

- [x] Run formatting, lint, build, unit, PostgreSQL integration, migration,
  security, and executable smoke checks; record blocked environment checks.
- [x] Run `node scripts/check-openspec-change-names.mjs` and
  `openspec validate --all --strict`.
- [x] Run `driftwatchdog gate`, inspect the complete diff, and verify no
  unrelated files or worktree changes were staged or modified.
