# application-model Specification

## Purpose
TBD - created by archiving change application-foundation-and-desired-state. Update Purpose after archive.
## Requirements
### Requirement: One application model covers all origins
The platform MUST represent generated, imported Git, imported local, template,
fork, and external projects as `Application` records with an `origin` value.

#### Scenario: Import and generated applications share identity
- **WHEN** one application is created from a prompt and another from a Git repository
- **THEN** both expose the same application-level fields and lifecycle collections
- **AND** only their origin metadata differs

### Requirement: Environments isolate operational state
Each application MUST support development, preview, and production environments,
with environment-scoped runtime, capability, resource, secret, deployment, and
domain references.

#### Scenario: Preview does not mutate production
- **WHEN** a preview environment changes a capability binding
- **THEN** the production binding remains unchanged

### Requirement: Portable manifests are optional
The platform MUST import and export a versioned `labrys.yaml` manifest, while
allowing imported applications to operate without that file in their repository.

#### Scenario: Existing project has no manifest
- **WHEN** an imported project lacks `labrys.yaml`
- **THEN** the platform uses its database state as source of truth
- **AND** it can later export a portable manifest

