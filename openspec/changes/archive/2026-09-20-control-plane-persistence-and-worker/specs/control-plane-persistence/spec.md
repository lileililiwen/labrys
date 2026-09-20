# control-plane-persistence Specification

## ADDED Requirements

### Requirement: Authoritative state survives process boundaries

The control plane MUST persist application, environment, desired state,
observed state, and lifecycle records in PostgreSQL through versioned SQLx
migrations, while preserving the existing application/environment isolation
and generation invariants.

#### Scenario: Process restart preserves desired and observed state

- **WHEN** the control-plane process restarts after a desired-state write and
  before the next reconciliation cycle
- **THEN** the next process reads the same desired and observed records
- **AND** a preview or development write cannot change production state

#### Scenario: Stale generation cannot overwrite newer state

- **WHEN** two writers submit updates from the same prior generation
- **THEN** exactly one update commits
- **AND** the other receives a conflict without changing the stored state

### Requirement: Persisted evidence is attributable and redacted

Events, logs, audit records, verification evidence, and usage records MUST be
durable, queryable by their existing correlation and resource identities, and
free of known secret values before insertion.

#### Scenario: Credential-bearing provider failure is stored safely

- **WHEN** a provider error contains a known secret value
- **THEN** the persisted diagnostic contains a redaction marker and recovery
  detail
- **AND** no persisted event, log, audit, or evidence field contains the
  secret value

#### Scenario: Audit ordering survives concurrent writes

- **WHEN** multiple platform actions append audit records concurrently
- **THEN** the database assigns one unambiguous sequence and chain order
- **AND** verification detects a missing or tampered record

### Requirement: Persistence mutations are transactionally coherent

A mutation that changes desired state and its attributable platform event MUST
commit together or leave both unchanged; observed-state and job transitions
MUST not partially acknowledge a failed transaction.

#### Scenario: Event write fails during desired-state mutation

- **WHEN** the event append cannot commit
- **THEN** the desired-state mutation is rolled back
- **AND** the next reconciliation observes the prior consistent state
