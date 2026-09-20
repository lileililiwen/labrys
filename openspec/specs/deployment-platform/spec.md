# deployment-platform Specification

## Purpose
TBD - created by archiving change deployment-and-rollback. Update Purpose after archive.
## Requirements
### Requirement: Production deployment is a first-class object
Every deployment MUST retain revision, build, image, runtime, environment,
resources, endpoint, health, logs, and rollback-target metadata.

#### Scenario: Container-only application deploys
- **WHEN** a project is buildable through the Generic Container Runtime
- **THEN** it can produce an OCI deployment
- **AND** deployment records remain language-independent

### Requirement: Health gates promotion
A deployment MUST NOT become healthy or receive production traffic until its
configured runtime and application health checks pass.

#### Scenario: New revision is unhealthy
- **WHEN** the new revision fails health checks
- **THEN** promotion stops and the deployment is failed or degraded
- **AND** the prior healthy rollback target remains identifiable

### Requirement: Rollback boundaries are explicit
Source, deployment, and configuration rollback MUST be independently requested;
deployment rollback MUST NOT claim to restore database data.

#### Scenario: Migration introduced incompatible data
- **WHEN** an operator rolls back the application image
- **THEN** the platform warns that database data is not automatically rolled back

