# control-plane-api-hardening Specification

## Purpose
TBD - created by archiving change control-plane-api-hardening. Update Purpose after archive.
## Requirements
### Requirement: Token auth supports rotation and revocation

The API MUST authenticate against multiple scoped tokens with expiry,
support rotation without restart, and enforce revocation on the next
request, keeping single-token env configuration as a warning bootstrap.

#### Scenario: Rotated token takes over without restart

- **WHEN** configuration adds a replacement token and revokes the old one
- **THEN** new requests succeed on the replacement and fail closed on
  the revoked token with recovery guidance

#### Scenario: Expired token is presented

- **WHEN** a request bears an expired token
- **THEN** the API denies it before any mutation with a recovery path
  and a redacted audit record

### Requirement: Actor identity binds to the credential

The request actor MUST be derived from the authenticated token identity,
denying header actors outside the token scope while preserving existing
approval boundaries and attribution shapes.

#### Scenario: Agent token claims human actor

- **WHEN** an agent-scoped token presents a human actor header
- **THEN** the request is denied before enqueueing provider work and the
  denial carries the approval recovery path

