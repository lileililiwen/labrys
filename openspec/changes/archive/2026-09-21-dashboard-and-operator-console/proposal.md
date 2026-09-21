# Proposal: Dashboard and operator console

## Why

The dashboard is only a model contract; no web application exists to show
application state, platform-vs-agent attribution, logs, health, approvals,
previews, deployments, or safe mutations.

## What Changes

- Build a Next.js/React operator console on the versioned control-plane API.
- Add application/environment overview, inspector/adoption findings, previews,
  capabilities/resources, deployments/rollback, logs/events, verification,
  approvals, and secret-reference views.
- Enforce read/write boundaries and make platform evidence distinct from agent
  claims throughout the UI.

## BFS Impact Map

- **Dependencies:** control-plane API/CLI and persisted/runtime evidence.
- **Affected:** all operator-visible resources, live job states, health,
  approval flows, and failure recovery.
- **Unaffected:** provider implementation and low-level container execution.
- **Accessibility/security:** keyboard navigation, clear status semantics,
  redacted secrets, safe confirmation for destructive writes.

## Capabilities

- `operator-dashboard`

## Non-goals

- Do not make the dashboard a source of truth; the API/database remain
  authoritative.
- Do not display secret values or claim health from agent text.
