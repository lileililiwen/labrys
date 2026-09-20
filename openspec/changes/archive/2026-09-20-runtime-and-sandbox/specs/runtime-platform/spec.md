# Runtime platform

## ADDED Requirements

### Requirement: Container compatibility is the minimum boundary
Any project with a valid Dockerfile, configured port, and optional healthcheck
MUST be eligible for build, run, preview, logs, and deployment workflows.

#### Scenario: Haskell container has no native adapter
- **WHEN** a Haskell project supplies a valid Dockerfile
- **THEN** the generic runtime builds and starts it
- **AND** the project is classified Tier 0 rather than rejected by language

### Requirement: Runtime profiles are explicit
The runtime MUST distinguish development and production profiles and MUST NOT
assume a dev server is a production command.

#### Scenario: ASP.NET Core runs in both profiles
- **WHEN** development is requested
- **THEN** the runtime may use `dotnet watch`
- **AND** production uses a reproducible build artifact or OCI image

### Requirement: Sandbox limits are enforced
Runtime execution MUST enforce configured CPU, memory, timeout, filesystem,
network, capability, and process-count limits.

#### Scenario: Build exceeds timeout
- **WHEN** a build exceeds its configured timeout
- **THEN** it is cancelled and marked failed
- **AND** the event includes the enforced limit and recovery detail

