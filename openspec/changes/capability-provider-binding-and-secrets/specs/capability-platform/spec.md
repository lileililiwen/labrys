# Capability platform

## ADDED Requirements

### Requirement: Capability provider and binding are distinct
The platform MUST represent what an application needs, who supplies it, and how
it is connected as separate versioned objects.

#### Scenario: PostgreSQL uses two framework bindings
- **WHEN** one application uses EF Core and another uses SQLx against the same provider type
- **THEN** both resolve `database.postgres` through distinct bindings
- **AND** provider identity remains independent of framework integration

### Requirement: Generic bindings preserve language neutrality
Every supported capability that lacks a framework binding MUST offer a generic
binding when a portable contract is possible.

#### Scenario: Rust project has no deep binding
- **WHEN** a Rust project requests PostgreSQL
- **THEN** it can receive a generic `DATABASE_URL` binding
- **AND** it is not blocked by the absence of a Rust-specific deep binding

### Requirement: Capability modes support gradual adoption
Managed, external, and adopted modes MUST be explicit and transitions MUST be
auditable and reversible where the provider supports reversal.

#### Scenario: Existing database becomes adopted
- **WHEN** a user approves adoption of an external PostgreSQL database
- **THEN** the platform records the source, approval, binding, and resulting mode
- **AND** it does not silently recreate or delete the existing database

### Requirement: Secret values never enter agent context
Secret values MUST be encrypted at rest, redacted from events and logs, and
injected only into authorized runtime or provider operations.

#### Scenario: Secret replacement is requested
- **WHEN** an agent requests replacement of a production secret
- **THEN** a human approval is required
- **AND** neither old nor new values appear in the agent transcript

