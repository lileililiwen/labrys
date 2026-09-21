# preview-execution Specification

## ADDED Requirements

### Requirement: Previews are isolated and revocable

Each preview MUST run in its session workspace, expose access only after
health, and revoke process and access state on expiry, failure, or explicit
revocation.

#### Scenario: Two sessions preview one application

- **WHEN** two sessions start previews concurrently
- **THEN** each uses an independent workspace and runtime
- **AND** neither can overwrite the other's preview state

#### Scenario: Preview expires

- **WHEN** a preview reaches its expiry time
- **THEN** its process is stopped and access is revoked
- **AND** later requests cannot use the old URL or QR payload
