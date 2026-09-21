# continuous-integration-and-gates Specification

## Purpose
TBD - created by archiving change ci-packaging-and-release-evidence. Update Purpose after archive.
## Requirements
### Requirement: CI runs all required quality boundaries

CI MUST run source checks, OpenSpec validation, migrations, unit/integration
tests, security checks, packaging checks, and applicable container/API/UI
checks on changes affecting those surfaces.

#### Scenario: Migration or worker code changes

- **WHEN** a change touches persistence or worker code
- **THEN** CI starts isolated PostgreSQL integration tests and migration checks
- **AND** a failure blocks the change

### Requirement: Gate failures are actionable

Every failed or unavailable check MUST expose the command, evidence artifact,
scope, and next action; unavailable infrastructure MUST NOT be reported as a
pass.

#### Scenario: Container runtime is unavailable

- **WHEN** the runtime integration service cannot start
- **THEN** the check is marked blocked with diagnostic evidence
- **AND** no release claim is emitted

