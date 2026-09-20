# Preview platform

## ADDED Requirements

### Requirement: Agent sessions are isolated
Each session MUST modify an isolated worktree and MUST expose its diff before
merge into the canonical repository.

#### Scenario: Two agents edit one application
- **WHEN** two sessions run concurrently
- **THEN** each has an independent worktree and preview environment
- **AND** neither can silently overwrite the other session's changes

### Requirement: Web preview is shareable only after health
The platform MUST issue a temporary preview URL only after the development
runtime starts and its health check reaches a usable state.

#### Scenario: Preview server fails health
- **WHEN** the dev server starts but `/health` fails
- **THEN** the preview remains unavailable
- **AND** the failure is visible in preview and agent events

### Requirement: Mobile preview identifies its transport
An Expo preview MUST expose project, server, expiry, and QR transport metadata,
and MUST distinguish Expo Go from development-build flows.

#### Scenario: User scans an Expo preview
- **WHEN** an active Expo Go preview is shared
- **THEN** the QR resolves to the session's development server
- **AND** the share record indicates its expiry and transport mode

