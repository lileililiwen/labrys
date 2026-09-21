# Proposal: Runtime tier depth expansion

## Why

The runtime boundary stops at Tier 2 framework layout while
`docs/release.md` marks Tier 3 (capability binding, code modification,
health, migration, preview, and production conventions deeply
integrated) as remaining roadmap scope, and ROADMAP v0.2 promises
broader Rust/Expo support plus Django/FastAPI/Axum integrations. The
detectors (`crates/labrys-core/src/runtime.rs`,
`crates/labrys-core/src/inspector.rs`) have native Node/.NET/Python/
Rust/Expo adapters with Tier 1–2 detection, but no deep framework
conventions, so supported projects fall back to generic handling for
migrations, test/dev-server commands, and health endpoints.

## What Changes

- Add Tier 3 integrations for Django, FastAPI, Axum, and deeper
  Rust/Expo coverage: framework layout, migration commands, test
  commands, dev-server conventions, production entrypoints, and
  health-endpoint defaults.
- Extend the inspector with framework-aware evidence (settings/modules,
  route/migration discovery) that corroborates or confines adapter
  claims through the existing confidence/unknown machinery.
- Extend runtime and inspector capability specs, test matrices, and the
  release tier-scope docs; generic Dockerfile fallback stays mandatory
  and unchanged.

## BFS Impact Map

- **Affected:** runtime adapters, inspector plugins/evidence, adoption
  plans, `labrys.yaml` portability notes, runtime/inspector tests,
  release tier-scope docs.
- **Dependencies:** none (extends pure-core contracts; executable
  dispatch consumes the new detections without change).
- **Security:** detection reads project files only, never executes
  project code; migration/test commands are proposed, never auto-run
  without approval; no new network or credential surface.
- **Unaffected:** container execution, provider adapters, secret cipher,
  API auth, dashboard, deployment/rollback semantics.

## Capabilities

- `runtime-tier-depth`

## Non-goals

- Do not add new language runtimes beyond deepening the existing five
  plus the three named frameworks.
- Do not auto-apply code modifications; adoption plans stay
  propose-only under approval gates.
- Do not change the generic-container Tier 0 minimum or its fallback
  position.
