# Agent platform

## ADDED Requirements

### Requirement: Agent backends are replaceable
The platform MUST support native and third-party AgentBackend implementations
through one session, prompt, interrupt, and resume contract.

#### Scenario: OpenCode and native sessions emit one event vocabulary
- **WHEN** equivalent actions occur through two backends
- **THEN** the platform records normalized events with backend identity
- **AND** consumers do not depend on backend-specific event names

### Requirement: High-risk actions require approval
Production deploys, destructive resource actions, destructive migrations,
secret replacement, domain changes, billing changes, and external writes MUST be
blocked until a human approval is recorded.

#### Scenario: Production deploy is proposed
- **WHEN** an agent requests production deployment
- **THEN** validation creates an approval request
- **AND** execution cannot start before approval

### Requirement: Secrets are not agent context
Agent events and prompts MUST contain secret references or redacted metadata,
never secret values.

#### Scenario: Runtime needs a client secret
- **WHEN** a binding requests a secret
- **THEN** the runtime receives an injected secret reference
- **AND** the agent stream contains only the reference identifier

