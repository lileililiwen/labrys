# secret-encryption-hardening Specification

## Purpose
TBD - created by archiving change secret-encryption-hardening. Update Purpose after archive.
## Requirements
### Requirement: Secrets are sealed with a real cipher

Secret values MUST be sealed with an authenticated cipher under an
external key in versioned envelopes, with authentication failures
surfaced as verification failures carrying recovery.

#### Scenario: Seal and reopen a value

- **WHEN** a value is sealed and reopened with the active key
- **THEN** the plaintext round-trips and any tampered envelope fails
  authentication without exposing plaintext

#### Scenario: Old envelope after rotation

- **WHEN** a pre-rotation envelope is opened
- **THEN** it dispatches on its version and remains readable until its
  reference is rotated

### Requirement: Rotation is auditable and value-free

Rotation MUST re-seal live references under the new key, require human
approval for production scopes, and record only references and version
lineage in the audit trail.

#### Scenario: Production rotation without approval

- **WHEN** rotation is requested on a production scope without a granted
  named approval
- **THEN** it is denied before any re-seal and the denial carries recovery

