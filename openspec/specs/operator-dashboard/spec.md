# operator-dashboard Specification

## Purpose
TBD - created by archiving change dashboard-and-operator-console. Update Purpose after archive.
## Requirements
### Requirement: Dashboard shows platform-owned application truth

The dashboard MUST show application, environment, capability, resource,
preview, deployment, log, verification, and health state from the API while
distinguishing platform evidence from agent claims.

#### Scenario: Agent reports success but deployment health fails

- **WHEN** the agent stream says deployment completed and platform health fails
- **THEN** the dashboard shows the platform failure and recovery path
- **AND** it does not display production as healthy

### Requirement: Dashboard writes preserve approval boundaries

Dashboard mutations MUST use API authorization and approval workflows for
deployments, capabilities, resources, domains, settings, and rollback.

#### Scenario: Operator opens rollback

- **WHEN** an operator selects a deployment rollback
- **THEN** the dashboard shows the database-data warning and approval scope
- **AND** no rollback begins until the API accepts named approval

### Requirement: Secrets are never rendered as values

Secret views MUST display references, versions, scope, and rotation state
without exposing secret values in the UI or client logs.

#### Scenario: Secret metadata is loaded

- **WHEN** the dashboard loads environment secrets
- **THEN** it shows safe references and status metadata
- **AND** the value is absent from rendered output and network-facing UI state

