# control-plane-api Specification

## Purpose
TBD - created by archiving change control-plane-api-and-cli. Update Purpose after archive.
## Requirements
### Requirement: API mutations are authorized and asynchronous

The API MUST enforce actor/environment scope, approval policy, and idempotency,
and MUST return durable job state for work that continues after the request.

#### Scenario: Agent requests production deploy

- **WHEN** an agent calls the production deploy endpoint without approval
- **THEN** the API rejects the mutation before enqueueing provider work
- **AND** the response contains a recovery path for human approval

#### Scenario: Client retries a mutation

- **WHEN** the same idempotency key is submitted twice
- **THEN** both responses identify one durable operation
- **AND** the side effect executes once

### Requirement: API responses are attributable and safe

Responses and request-linked events MUST include correlation and resource
identity, structured failure recovery, and no secret values.

#### Scenario: Deployment is still progressing

- **WHEN** a deployment request is accepted but health is not observed
- **THEN** the API reports an in-progress state and job reference
- **AND** it does not report production healthy

