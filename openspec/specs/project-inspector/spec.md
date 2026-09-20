# project-inspector Specification

## Purpose
TBD - created by archiving change inspector-and-adoption. Update Purpose after archive.
## Requirements
### Requirement: Deterministic evidence precedes inference
The inspector MUST examine Docker and project configuration evidence before
using agent inference, and MUST retain the evidence source for each finding.

#### Scenario: Dockerfile identifies an unknown language
- **WHEN** a project contains a valid Dockerfile but no recognized manifest
- **THEN** the inspector emits a container-compatible profile
- **AND** records language and framework as unknown rather than inventing them

### Requirement: Adoption is incremental and reviewable
The inspector MUST produce an Adoption Plan that distinguishes KEEP, ADOPT, and
OPTIONAL MIGRATION items without changing the project automatically.

#### Scenario: Existing OAuth is preserved
- **WHEN** an imported project contains configured Google OAuth
- **THEN** the plan recommends KEEP for that auth integration
- **AND** may recommend ADOPT for secrets, preview, deployment, and logs

### Requirement: Direct rewrites are prohibited by default
The platform MUST require an inspect, understand, model, propose, and apply
sequence before an agent performs broad changes to an imported project.

#### Scenario: Agent proposes a migration
- **WHEN** an agent wants to replace existing storage
- **THEN** the platform presents the proposal and affected evidence
- **AND** does not apply it without explicit approval

