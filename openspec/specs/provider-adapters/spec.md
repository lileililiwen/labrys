# provider-adapters Specification

## Purpose
TBD - created by archiving change provider-registry-and-domain-adapters. Update Purpose after archive.
## Requirements
### Requirement: Provider operations are scoped and observable

Provider adapters MUST execute capability operations with application,
environment, resource, approval, idempotency, and correlation scope, and MUST
persist platform-owned readiness observations.

#### Scenario: Capability provisioning is accepted but not ready

- **WHEN** a provider accepts a PostgreSQL provisioning request
- **THEN** the resource remains provisioning
- **AND** only a later platform observation can make it ready

#### Scenario: Provider returns credential material

- **WHEN** a provider error includes a credential
- **THEN** persisted events and diagnostics redact it
- **AND** the operation exposes recovery guidance

### Requirement: Destructive provider actions require approval

External resource deletion, replacement, migration, and adoption changes MUST
be blocked without a named human approval matching the target scope.

#### Scenario: Agent requests database deletion

- **WHEN** an agent requests deletion without approval
- **THEN** no provider call occurs
- **AND** the denial is attributable and recoverable

