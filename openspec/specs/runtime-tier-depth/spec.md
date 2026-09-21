# runtime-tier-depth Specification

## Purpose
TBD - created by archiving change runtime-tier-depth-expansion. Update Purpose after archive.
## Requirements
### Requirement: Frameworks detect with deep conventions

Django, FastAPI, Axum, Rust, and Expo projects MUST detect with
framework layout, migration, test, dev-server, production, and health
conventions expressed as calibrated, evidenced claims.

#### Scenario: Django project is inspected

- **WHEN** a Django project with settings, manage.py, and migrations is
  detected
- **THEN** planning yields migration, test, dev, production, and health
  conventions with file evidence and a non-generic adapter decision

#### Scenario: Conflicting framework signals

- **WHEN** rival high-confidence framework claims conflict
- **THEN** the conflict becomes an explicit unknown with candidate facts
  preserved instead of a false detection

### Requirement: Generic fallback never regresses

Any project with a valid Dockerfile MUST still build and run via the
generic adapter regardless of framework detection outcomes.

#### Scenario: Unknown framework with valid Dockerfile

- **WHEN** no framework adapter claims the project but a valid Dockerfile
  exists
- **THEN** the generic adapter handles build/run and Tier 0 guarantees
  hold

