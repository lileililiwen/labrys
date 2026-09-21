# registry-and-domain-delivery Specification

## Purpose
TBD - created by archiving change provider-registry-and-domain-adapters. Update Purpose after archive.
## Requirements
### Requirement: Production artifacts are digest verified

The platform MUST push only verified production OCI artifacts and MUST refuse
deployment when the registry-reported digest differs from the recorded digest.

#### Scenario: Registry digest differs

- **WHEN** a push returns a digest different from the build artifact
- **THEN** deployment remains unpromoted
- **AND** the mismatch is recorded with recovery guidance

### Requirement: Domains route only to promoted healthy deployments

DNS/TLS configuration and traffic attachment MUST be separately observable,
approval-gated where destructive, and unable to target an unpromoted or
unhealthy deployment.

#### Scenario: Certificate issuance fails

- **WHEN** TLS issuance fails for an approved domain
- **THEN** traffic is not attached
- **AND** the domain reports the certificate failure rather than healthy

