# executable-dispatch-wiring Specification

## ADDED Requirements

### Requirement: Daemon executes jobs through real executors

The `control-plane` daemon MUST dispatch claimed jobs through the real
container, preview, and provider executors selected from process
configuration, persisting platform observations, separated logs, and
evidence with secret redaction.

#### Scenario: Production deploy job with runtime available

- **WHEN** a deploy job is claimed and a container runtime is reachable
- **THEN** the daemon builds, runs, and health-checks via the bounded
  executor, persists the observation, and finalizes the job from platform
  evidence only

#### Scenario: Container runtime is absent

- **WHEN** no container runtime is reachable at startup or per execution
- **THEN** the daemon reports an environment blocker with the recovery
  action and performs no simulated execution pass

### Requirement: Staging smoke proves the executable loop

Staging smoke MUST exercise import → build → run → health → preview →
cleanup against the disposable environment and MUST keep unavailable
infrastructure BLOCKED with recovery instead of passing.

#### Scenario: Disposable environment is provisioned

- **WHEN** PostgreSQL and Docker are available to the smoke run
- **THEN** a preview URL is issued only after platform-owned health and
  revoked on expiry, with all stages recorded as pass

#### Scenario: Docker is unavailable to smoke

- **WHEN** no Docker daemon is reachable
- **THEN** execution stages are recorded blocked with the provisioning
  next action and no release claim is emitted
