# release-packaging-and-evidence Specification

## ADDED Requirements

### Requirement: Releases are reproducible and inspectable

Release artifacts MUST identify source revision, protocol versions, dependency
metadata, migration version, checksums, and container/image digests.

#### Scenario: Operator inspects a release

- **WHEN** a release artifact is downloaded
- **THEN** its checksum and provenance can be verified
- **AND** the supported runtime/provider scope is explicit

### Requirement: Runtime claims require runtime evidence

The release report MUST distinguish model/unit, integration, staging, and
production evidence and MUST not claim a lifecycle path from static tests alone.

#### Scenario: Model tests pass without Docker

- **WHEN** all pure-core tests pass but container integration is unavailable
- **THEN** the release remains incomplete for runtime delivery
- **AND** the report records the missing evidence and recovery action
