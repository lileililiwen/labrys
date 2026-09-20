# reconciliation Specification

## Purpose
TBD - created by archiving change controllers-and-resource-lifecycle. Update Purpose after archive.
## Requirements
### Requirement: Desired and actual state are separate
Controllers MUST compare persisted desired state with provider/runtime observed
state and MUST be able to reconcile after the initiating request has ended.

#### Scenario: Database disappears after a successful request
- **WHEN** desired PostgreSQL is ready but the observed resource is missing
- **THEN** the resource controller enqueues an idempotent provisioning action
- **AND** the application reports the mismatch until recovery is observed

### Requirement: Resource lifecycle is explicit
Resources MUST expose requested, provisioning, ready, degraded, failed, deleting,
and deleted states with transition evidence.

#### Scenario: Provisioning fails
- **WHEN** a provider returns a terminal provisioning error
- **THEN** the resource becomes failed with a redacted reason and retry policy
- **AND** dependent bindings do not claim healthy state

### Requirement: Agent completion cannot replace reconciliation
An agent tool result MUST NOT be treated as proof of ready or healthy state.

#### Scenario: Tool returns before provider readiness
- **WHEN** a provisioning tool returns an accepted request
- **THEN** the capability remains provisioning until a controller observes readiness

