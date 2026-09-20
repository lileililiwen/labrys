# reconciliation-worker Specification

## Purpose
TBD - created by archiving change control-plane-persistence-and-worker. Update Purpose after archive.
## Requirements
### Requirement: Jobs are durable and idempotent

The worker MUST claim persisted controller jobs with a recoverable lease,
enforce idempotency keys, and record completion, retry, pause, dead-letter, and
requeue transitions durably.

#### Scenario: Two workers claim one job

- **WHEN** two workers concurrently poll the same ready job
- **THEN** exactly one worker claims it
- **AND** the other worker does not execute the side effect

#### Scenario: Duplicate request arrives while work is active

- **WHEN** a job with an existing active or terminal idempotency key is
  submitted
- **THEN** the queue returns the existing job outcome
- **AND** it does not enqueue or execute a second side effect

### Requirement: Interrupted jobs recover without false readiness

The worker MUST make an expired lease claimable again with bounded retry
metadata and MUST keep resources non-ready until a platform-owned observation
proves readiness.

#### Scenario: Worker exits during provider execution

- **WHEN** a worker stops after claiming a job but before recording completion
- **THEN** the expired lease is recovered by another worker
- **AND** the resource remains provisioning or degraded until readiness is
  observed

#### Scenario: Agent claims completion before provider readiness

- **WHEN** an agent result is stored before the provider health observation
- **THEN** the worker records the claim as non-authoritative
- **AND** reconciliation does not promote the resource or deployment

### Requirement: Shutdown and failure paths are observable

The worker MUST attribute claim, attempt, retry, terminal failure, pause,
dead-letter, recovery, and shutdown transitions to a worker identity and store
redacted failure and recovery details.

#### Scenario: Retry limit is exhausted

- **WHEN** a job fails after its configured retry limit
- **THEN** it enters a durable dead-letter state with redacted reason,
  attempt history, and explicit requeue guidance
- **AND** health does not report the dependent resource as ready

