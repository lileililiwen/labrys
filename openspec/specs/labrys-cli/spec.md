# labrys-cli Specification

## Purpose
TBD - created by archiving change control-plane-api-and-cli. Update Purpose after archive.
## Requirements
### Requirement: CLI exposes the stable lifecycle contract

The CLI MUST expose the modeled import, inspect, run, agent, preview,
capability, resource, build, deploy, logs, health, rollback, and doctor
commands with machine-readable envelopes and actionable failures.

#### Scenario: Operator diagnoses an imported project

- **WHEN** the operator runs import, inspect, and doctor
- **THEN** each command returns structured application findings
- **AND** failures identify the next safe action

### Requirement: CLI preserves actor and secret boundaries

The CLI MUST prevent agent actors from human-only mutations and MUST render
secret references or redaction markers rather than secret values.

#### Scenario: Agent invokes rollback

- **WHEN** an agent invokes rollback without named human approval
- **THEN** the command fails without enqueuing rollback
- **AND** the output states the approval requirement

