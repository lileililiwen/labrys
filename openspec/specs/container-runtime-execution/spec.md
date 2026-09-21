# container-runtime-execution Specification

## Purpose
TBD - created by archiving change container-execution-and-preview-runtime. Update Purpose after archive.
## Requirements
### Requirement: Generic container projects execute under enforced limits

The platform MUST build and run a valid Dockerfile project with explicit
sandbox limits, cancellation, health, logs, and cleanup.

#### Scenario: Generic project builds and runs

- **WHEN** a project has a valid Dockerfile and configured port
- **THEN** the executor builds and starts it under the requested limits
- **AND** platform-owned health and logs are recorded

#### Scenario: Build exceeds its timeout

- **WHEN** a build exceeds its configured timeout
- **THEN** the process is cancelled and marked failed
- **AND** evidence contains the limit and recovery detail without secrets

### Requirement: Runtime failures cannot masquerade as readiness

The platform MUST keep a runtime non-ready until its configured health check
passes and MUST distinguish development health from production health.

#### Scenario: Development server returns an unusable health response

- **WHEN** a preview process starts but its health check fails
- **THEN** no preview URL is issued
- **AND** the failure remains visible to controllers and operators

