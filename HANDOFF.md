current_spec: deployment-and-rollback

# Labrys handoff

## Current state

`application-foundation-and-desired-state` is implemented, locally verified,
and ARCHIVED as `openspec/changes/archive/2026-09-20-application-foundation-and-desired-state`,
`inspector-and-adoption` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-20-inspector-and-adoption`, and
`agent-protocol-and-policy` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-20-agent-protocol-and-policy`,
`runtime-and-sandbox` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-20-runtime-and-sandbox`, and
`workspace-and-preview` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-20-workspace-and-preview`, and
`capability-provider-binding-and-secrets` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-20-capability-provider-binding-and-secrets`,
and `controllers-and-resource-lifecycle` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-20-controllers-and-resource-lifecycle`,
with canonical
specs promoted to `openspec/specs/application-model/spec.md`,
`openspec/specs/project-inspector/spec.md` (3 requirements),
`openspec/specs/agent-platform/spec.md` (3 requirements),
`openspec/specs/runtime-platform/spec.md` (3 requirements),
`openspec/specs/preview-platform/spec.md` (3 requirements),
`openspec/specs/capability-platform/spec.md` (4 requirements), and
`openspec/specs/reconciliation/spec.md` (3 requirements).
The `labrys-core` crate now holds a deterministic `Inspector`
(`crates/labrys-core/src/inspector.rs`): `INSPECTION_ORDER` (dockerfile →
compose → manifest → framework_convention → configuration → ci → source,
plugins last), `ProjectSnapshot`, `Evidence`/`Fact`/`Unknown`,
`ApplicationProfile`, `InspectorPlugin` registration with confidence/evidence
handling (agreement corroborates, high-confidence conflict becomes explicit
unknown, low-confidence rival kept as candidate fact), `AdoptionPlan` with
KEEP/ADOPT/OPTIONAL MIGRATION generation that never applies, `ExplicitApproval`
gating, and `BroadChangeGate` enforcing inspect → understand → model → propose
→ apply; plus a versioned agent protocol (`crates/labrys-core/src/agent.rs`):
`AGENT_PROTOCOL_VERSION` (1), `AgentBackend` session/prompt/interrupt/resume
contract, normalized event vocabulary (`NORMALIZED_EVENT_NAMES`, 14 names),
platform-owned `AgentSession` policy boundary (approval gating for 7 high-risk
actions, production raw-shell denial, secret-leak scanning, timeout,
interrupt/resume replay), `ToolTier` precedence (Platform API → framework →
generic → raw shell), `CapabilityRequest`/`ResourceRequest` envelopes carrying
only secret references, `NativeAgent` Plan → Act → Observe → Verify reference
flow, and `ExternalProcessAgent` adapter with backend-name normalization.
The `labrys-core` crate now holds a deterministic runtime model
(`crates/labrys-core/src/runtime.rs`): `RuntimeAdapter` detect/prepare
contract with `GenericAdapter` Tier 0 fallback (any valid Dockerfile with a
`FROM` line builds/runs regardless of language), native `Node`/`DotNet`/
`Python`/`Rust`/`Expo` adapters with Tier 1 base and Tier 2 framework-layout
detection, `RuntimeConfig` planning with explicit development/production
profiles (dev never mints OCI, prod always does; `dotnet watch` never reused
as the production command), Docker `SandboxLimits` defaults (1 vCPU, 512 MiB,
600 s timeout, read-only root, isolated network, drop `ALL`, 128 pids) with
pre-schedule validation, deterministic `simulate_build` (timeout cancels and
marks failed with limit + recovery detail), `enforce_run_usage` process/memory
caps, `Endpoint` port-exposure tied to network posture, and `evaluate_health`
semantics that keep dev-server 3xx from masquerading as production healthy.
The `labrys-core` crate now holds a deterministic workspace and preview model
(`crates/labrys-core/src/workspace.rs`): `CanonicalRepository` +
`SessionWorkspace::open` deriving per-session worktree paths and branches so
two sessions on one application can never share a checkout, `WorkspaceDiff`
exposure with `merge_ready`/`MergeRequest::propose` gating merge on a reviewed
non-empty diff plus named `ExplicitApproval`, `RollbackInfo` base-revision
metadata, `WorkspaceRegistry` duplicate-allocation rejection and
`cleanup_finished` for merged/discarded worktrees, `WebPreview` health-gated
temporary URLs (URL only while `Available`, `Unavailable`/`Expired` revoke
access, build failures carry limit + recovery detail into
`to_event_detail`), `ExpoShare` with `discover_server_url`, QR payloads
binding project/session/transport, Expo Go versus development-build
`ExpoTransport` distinction, expiry/revocation lifecycle, and `Feedback`
association that rejects cross-workspace preview or share attachment; plus a
deterministic capability and secrets model
(`crates/labrys-core/src/capability.rs`): versioned `Capability` (with
dependencies), `Provider` (capability supply, supported modes, reversal
support), and `Binding` (`Framework`/`Generic` kinds with a portable
`EnvContract` of literal/secret-ref/resource-field sources) as separate
objects; `resolve_binding` with framework-first selection, generic fallback,
and provider-mismatch/missing/conflict rejection; generic contracts for
PostgreSQL, object storage, auth, and file storage; `BoundCapability` with
managed/external/adopted `ModeTransition` history that requires a granted,
named `ExplicitApproval`, records adoption source, and refuses reversal the
provider cannot support; and a `SecretStore` sealing values under an external
`MasterKey` (XOR keystream stand-in for encrypted PostgreSQL) with
`redact` stripping known plaintext from events/logs, grant-scoped `inject`
and `inject_binding` (foreign application or environment rejected),
versioned `rotate`, `replace` that requires human approval for production
scopes while neither old nor new value enters the audit record, and
`delete`; plus a deterministic controller and reconciliation model
(`crates/labrys-core/src/controller.rs`): the `Controller` trait
(reconcile against a shared `JobQueue`, repeatable outside any request)
implemented by `ApplicationController` (desired vs observed `DesiredState`/
`ObservedState`), `ResourceController` (missing-resource drift → idempotent
provisioning, deletion convergence), `CapabilityController` (agent completion
claims counted, never trusted; readiness only via `observe_readiness`),
`PreviewController`, and `DeploymentController`; explicit `ResourcePhase`
lifecycle (requested → provisioning → ready | degraded | failed → deleting →
deleted) with validated transition evidence and generation-scoped
convergence versions; `JobQueue` as the MVP PostgreSQL jobs/worker boundary
(`IdempotencyKey` duplicate suppression while in flight, exponential
`RetryPolicy` backoff with `max_delay` cap, pause/resume, dead-letter +
explicit requeue recovery); and `HealthReport`/`aggregate_health` over
platform-owned checks only (Failed > Degraded > Unknown > Provisioning >
Paused > Healthy precedence, agent-sourced claims excluded and listed as
ignored) with `redact_reason` keeping secret values out of failure evidence.
98 tests pass (10 foundation + 9 inspector-adoption + 10 agent-protocol in
`crates/labrys-core/tests/agent_protocol.rs` + 13 runtime-sandbox in
`crates/labrys-core/tests/runtime_sandbox.rs` + 15 workspace-preview in
`crates/labrys-core/tests/workspace_preview.rs` + 22 capability-secrets in
`crates/labrys-core/tests/capability_secrets.rs` + 19 reconciliation-
controllers in `crates/labrys-core/tests/reconciliation_controllers.rs`). No CI, deployment,
or production evidence exists yet. The active queue remains a delegated
implementation handoff, not a claim of delivered product behavior. The pre-existing local Gate
blocker is FIXED: `.ai-gate/gate.yaml` no longer carries the rejected `notes`
field or `BLOCKED` blocking entry, declares `commands` for all 8 checks, and
`driftwatchdog gate` now reports PASS.

## Next change

Implement only `deployment-and-rollback` (ROADMAP Phase 3, item 8) after
reviewing its proposal, design, tasks, and capability scenarios.
Keep the pointer above in sync with `openspec list`; never use `none` or `TBD`.

## Verification evidence

- `controllers-and-resource-lifecycle` tasks.md — 5/5 checked from
  implementation evidence.
- `openspec/changes/archive/2026-09-20-controllers-and-resource-lifecycle`
  — archived with specs promoted (`reconciliation: create`, +3); canonical
  spec at `openspec/specs/reconciliation/spec.md` (3 requirements).
- `node scripts/check-openspec-change-names.mjs` — PASS; all active names are valid.
- `openspec list` — PASS; 3 active changes (foundation + inspector + agent +
  runtime + workspace + capability + controllers archived, next is
  `deployment-and-rollback` at 0/5).
- `openspec validate --all --strict` — PASS; 10 items passed, 0 failed
  (3 active changes + promoted `spec/agent-platform` + `spec/application-model`
  + `spec/capability-platform` + `spec/preview-platform` +
  `spec/project-inspector` + `spec/reconciliation` + `spec/runtime-platform`).
- `git diff --check` — PASS; no whitespace errors reported.
- `cargo build --workspace` — PASS; `cargo test --workspace` — PASS (98/98:
  10/10 in `crates/labrys-core/tests/application_foundation.rs`, 9/9 in
  `crates/labrys-core/tests/inspector_adoption.rs`, 10/10 in
  `crates/labrys-core/tests/agent_protocol.rs`, 13/13 in
  `crates/labrys-core/tests/runtime_sandbox.rs`, 15/15 in
  `crates/labrys-core/tests/workspace_preview.rs`, 22/22 in
  `crates/labrys-core/tests/capability_secrets.rs`, 19/19 in
  `crates/labrys-core/tests/reconciliation_controllers.rs`).
- `cargo fmt --all --check` — PASS; `cargo clippy --workspace --all-targets -- -D warnings` — PASS.
- `cargo audit` — PASS; no vulnerabilities reported.
- Local Gate (`driftwatchdog gate` in repo root) — PASS (8/8 checks:
  build, format, lint, openspec_change_names, openspec_strict_validation,
  repository_integrity, security, tests).
- Prior history: foundation archived with one non-blocking proposal warning
  (missing Why/What Changes headers); runtime archived with the same
  non-blocking proposal warning. Gate `notes`/`BLOCKED` schema fix and
  `driftwatchdog init` recorded in the previous handoff revision.

## Handoff lifecycle

Select one active change, set `current_spec`, implement and verify it, and
archive only after local Gate and strict validation pass. Each spec change ends
with exactly two commits: commit 1 holds the implementation, tests, archived
change, and promoted specs; commit 2 is the separate `HANDOFF.md` pointer
update to the next active change. Stop after commit 2. If no active changes
remain, remove the pointer.
