# verification-and-observability Specification

## Purpose
TBD - created by archiving change observability-and-verification. Update Purpose after archive.
## Requirements
### Requirement: Platform actions are attributable
Agent and platform events MUST identify actor, timestamp, application,
environment, action, resource, before, after, and result where applicable.

#### Scenario: Agent requests a capability
- **WHEN** a capability request is accepted or rejected
- **THEN** the event identifies the agent session and platform decision
- **AND** the relevant application and environment are queryable

### Requirement: Completion requires independent evidence
The verifier MUST evaluate applicable deterministic, build/test, health, schema,
browser, advisory, and human stages independently of an agent's completion text.

#### Scenario: Agent says Google login is done
- **WHEN** the agent reports completion
- **THEN** verification checks capability readiness, callback reachability,
  build state, and auth endpoint health
- **AND** a failed check prevents a successful completion result

### Requirement: Secret-bearing evidence is redacted
Logs, events, audit records, and verifier evidence MUST redact secret values and
provide safe references for diagnosis.

#### Scenario: Provider returns a credential error
- **WHEN** a provider error contains credential material
- **THEN** persisted evidence contains a redacted diagnostic
- **AND** operators can still identify the failing resource and recovery path

