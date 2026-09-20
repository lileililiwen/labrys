# Design

## Boundary and ownership

`labrys-core` remains the authoritative contract and policy layer. A new
control-plane crate owns SQLx adapters, migrations, transaction orchestration,
configuration, and worker execution. It may depend on `labrys-core` but the
core crate must not depend on SQLx, Tokio, a database driver, or process I/O.

The first executable is a worker-oriented control-plane process, not the final
HTTP API or CLI. It loads a database URL and safe runtime settings from
process configuration, runs migrations explicitly, and starts one or more
reconciliation workers. Provider/runtime execution is represented by injected
traits so this change cannot accidentally turn simulation into Docker
execution.

## Persistence model

Use PostgreSQL with SQLx compile-time checked queries where practical and
versioned migrations committed with the crate. Store the canonical application
aggregate and environment-scoped desired/observed state in normalized tables;
store versioned JSON snapshots only where the existing contract is extensible.
Use database constraints for application/environment ownership, generation
monotonicity, idempotency uniqueness, job uniqueness, and audit sequence
ordering.

Every mutation that changes desired state also records its attributable event
in one transaction. Observed state, health, logs, verification evidence, and
usage are written through separate repository methods so a failed observation
cannot silently rewrite desired state. Secret-bearing values are rejected or
redacted before SQL execution, not only when read back.

## Durable jobs and worker

Implement the existing `JobQueue` semantics with a PostgreSQL claim protocol:
claim uses a lease and `FOR UPDATE SKIP LOCKED`, records attempt and worker
identity, and makes idempotency keys unique while work is active or terminal.
Completion, retry scheduling, pause/resume, and dead-letter transitions are
transactional. Lease expiry makes an interrupted job claimable again with a
bounded recovery detail.

The worker invokes controller operations through an injected dispatcher,
records platform-owned observations, and never marks a resource ready from an
agent result. Backoff uses persisted timestamps rather than process sleep
state. Shutdown stops new claims, allows a bounded in-flight drain, and leaves
unfinished leases recoverable.

## Compatibility and verification

The in-memory repository remains available for deterministic unit tests. SQLx
integration tests run migrations on an isolated PostgreSQL database and cover
two workers competing for one job, restart after a lease, duplicate
idempotency, transaction rollback, redaction, and desired/observed separation.
The executable has a smoke test proving migrations and one recovery cycle;
this is infrastructure evidence, not production deployment evidence.
