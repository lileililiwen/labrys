# platform-surfaces Specification

## Purpose
TBD - created by archiving change cli-dashboard-and-plugin-sdk. Update Purpose after archive.
## Requirements
### Requirement: CLI exposes the application lifecycle
The CLI MUST provide stable commands for import, inspect, run, agent, preview,
capabilities, resources, build, deploy, logs, health, rollback, and diagnosis.

#### Scenario: Imported project is diagnosed
- **WHEN** a user runs `labrys import .`, `labrys inspect`, and `labrys doctor`
- **THEN** each command returns structured findings tied to the application
- **AND** failures include actionable recovery information

### Requirement: Plugins use a process protocol
Third-party extensions MUST be able to run outside the Rust control plane using
versioned JSON-RPC over stdin/stdout with explicit handshake and error behavior.

#### Scenario: TypeScript inspector plugin connects
- **WHEN** a compatible inspector process completes the handshake
- **THEN** the control plane can invoke it through the InspectorProvider contract
- **AND** a protocol mismatch prevents activation without crashing the control plane

### Requirement: Dashboard exposes ownership and health
The dashboard MUST distinguish agent actions from platform actions and show
application, environment, capability, resource, preview, deployment, logs,
secrets, and health state.

#### Scenario: Deployment fails after agent success
- **WHEN** the agent stream reports completion but deployment health fails
- **THEN** the dashboard shows the platform failure and evidence source
- **AND** it does not display the application as production healthy

