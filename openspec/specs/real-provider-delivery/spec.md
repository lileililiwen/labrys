# real-provider-delivery Specification

## Purpose
TBD - created by archiving change real-provider-delivery-adapters. Update Purpose after archive.
## Requirements
### Requirement: Real adapters provision behind existing traits

Provisioning, registry push, and domain delivery MUST execute through
real adapters implementing the existing provider traits, persisting
observations and evidence with credentials redacted.

#### Scenario: PostgreSQL provisioning succeeds

- **WHEN** a database provision operation runs with valid configuration
- **THEN** the adapter provisions with least-privilege credentials, the
  runtime persists a Ready observation, and no credential value appears
  in events, logs, or evidence

#### Scenario: Registry digest mismatches

- **WHEN** a pushed image digest does not match the expected digest
- **THEN** delivery is refused with a recovery-bearing failure and no
  traffic is attached

### Requirement: Destructive delivery requires approval

Deletion and traffic attachment MUST be denied without a granted, named
approval before any provider call, persisting the denial with zero
provider side effects.

#### Scenario: Delete requested without approval

- **WHEN** a delete operation arrives without a granted named approval
- **THEN** the runtime denies it before any adapter call and persists the
  denial with recovery guidance

