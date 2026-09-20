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
and `deployment-and-rollback` is implemented, locally verified, and ARCHIVED
as
`openspec/changes/archive/2026-09-20-deployment-and-rollback`,
and `observability-and-verification` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-20-observability-and-verification`,
and `cli-dashboard-and-plugin-sdk` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-20-cli-dashboard-and-plugin-sdk`,
and `control-plane-persistence-and-worker` is implemented, locally verified,
and ARCHIVED as
`openspec/changes/archive/2026-09-20-control-plane-persistence-and-worker`,
with canonical
specs promoted to `openspec/specs/application-model/spec.md`,
`openspec/specs/project-inspector/spec.md` (3 requirements),
`openspec/specs/agent-platform/spec.md` (3 requirements),
`openspec/specs/runtime-platform/spec.md` (3 requirements),
`openspec/specs/preview-platform/spec.md` (3 requirements),
`openspec/specs/capability-platform/spec.md` (4 requirements),
`openspec/specs/reconciliation/spec.md` (3 requirements),
`openspec/specs/deployment-platform/spec.md` (3 requirements),
`openspec/specs/verification-and-observability/spec.md` (3 requirements),
`openspec/specs/platform-surfaces/spec.md` (3 requirements),
`openspec/specs/control-plane-persistence/spec.md` (3 requirements), and
`openspec/specs/reconciliation-worker/spec.md` (3 requirements).
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
ignored) with `redact_reason` keeping secret values out of failure evidence;
plus a deterministic deployment and rollback model
(`crates/labrys-core/src/deployment.rs`): the language-independent
`Deployment` record retaining revision (`Revision` with config/capability
hashes and migration flag), `BuildResult`, OCI `Image` (digest-addressable,
`Generic`/`Native` source), `RuntimeConfig`, environment, resources,
`Endpoint`, `HealthStatus`, redacted `DeploymentLog`s, and `rollback_target`
with a validated `DeploymentPhase` graph
(pending → building → deploying → healthy | degraded | failed → superseded |
rolled_back) where only a platform `Healthy` observation promotes and
`Starting`/`Unknown` keep the rollout in `deploying`; `build_image` minting
OCI artifacts from generic and native production builds (development never
mints, non-OCI runtimes and failed/cancelled builds rejected, deterministic
revision-scoped digests); `Rollout` history wiring the prior healthy or
superseded revision as the next rollback target with promotion that
supersedes its predecessor; `plan_rollback`/`execute_rollback` for
independent source, deployment, and configuration rollbacks where a
deployment rollback always carries `DATABASE_DATA_WARNING` (never claiming
database restoration) plus a migration-in-delta escalation, requires a
granted named `ExplicitApproval`, and re-points domains and current traffic;
`select_rollback_target` refusing unbuilt or failed revisions; and
`attach_domain` approval-gating domain changes so traffic can never route to
an unpromoted deployment; plus a deterministic observability and verification
model (`crates/labrys-core/src/observability.rs`): `PlatformEvent`/`EventDraft`
attributing every action to an `EventActor` (agent session, platform, human,
provider) with `Correlation` trace/session/application/environment ids,
`EventAction` vocabulary, `EventResource`, before/after state, and
`EventResult`, appended through `EventLog` with redaction of all free-text
fields and per-application/environment/session/trace queries; `LogStore`
keeping agent, build, runtime, deployment, capability, and resource streams
separated with redacted messages; `HealthSnapshot` with a ttl whose
`effective_state` falls back to `Unknown` once stale; `UsageLedger`/
`UsageTotals` per-application metering; an append-only hash-chained `AuditLog`
(`AuditRecord::compute_hash`, `restore`, `verify`) that detects tampering,
chain breaks, and sequence gaps; and a `Verifier` recording the agent claim as
non-authoritative while evaluating `VerifierStage` deterministic, build/test,
health, schema, browser, advisory, and human results — later passes cannot
erase a stored failure (kept as a disagreement), advisory never blocks,
applicable stages without evidence yield `VerificationVerdict::Incomplete`,
`failures()` expose redacted `FailureExplanation`s naming the resource and
recovery path, and `EvidenceStore`/`RetentionPolicy` prune aged evidence while
always retaining unresolved blocking failures; plus deterministic product
surfaces (`crates/labrys-core/src/surfaces.rs`,
`crates/labrys-core/src/plugin.rs`): 15 stable `CliCommand`s returning
machine-readable `CliResponse` envelopes of structured `Finding`s with
recovery guidance, mutating commands gated on an idempotency key whose replay
never repeats the side effect, and `Cli::authorize` actor boundaries (agents
barred from lifecycle mutations, rollback human-only plus named approval,
production deploy human-only), with `doctor` aggregating health staleness,
plugin reports, and failed evidence; a versioned JSON-RPC plugin boundary
(`PLUGIN_PROTOCOL_VERSION`) with `PluginManifest`/`Handshake` negotiation,
`PluginFamily` required-method contracts, `PluginProcess` refusal for version
or contract mismatch and structured `RpcErrorCode` errors for unavailable,
stopped, unknown-method, and cancelled calls (never a panic), `PluginRegistry`
one-active-plugin-per-family with a `doctor`-ready `report()`, frame
`encode_response`/`decode_response`, and six `sdk` example plugins covering
`AgentProvider`, `CapabilityProvider`, `BindingProvider`, `RuntimeProvider`,
`InspectorProvider`, and `DeployProvider`; and dashboard `DashboardSection`
read/write boundaries (secrets, logs, and code changes read-only; deployments,
capabilities, resources, and settings approval-gated; agents barred) with
`render_dashboard` keeping agent and platform attribution separate so
production health is computed only from platform evidence and secrets render
as references only. A new `labrys-control-plane` crate
(`crates/labrys-control-plane/`) turns those contracts into a durable,
executable control plane without making the pure core depend on I/O: versioned
PostgreSQL migrations (`migrations/`) for the application aggregate,
environment-scoped desired/observed state, resource/deployment/capability
lifecycle, events, logs, a hash-chained append-only audit trail, evidence,
usage, and jobs; `PgApplicationStore` with transactional, optimistic
generation-checked desired-state writes that commit with their attributable
event and audit record and separate observed-state writes so a failed
observation never rewrites desired state and a preview/development write can
never touch production; redacting `PgEventStore`/`PgLogStore`/`PgAuditLog`/
`PgEvidenceStore`/`PgUsageLedger` that reject plaintext secrets before SQL and
assign audit sequence/chain under a transaction advisory lock; a durable
`PgJobQueue` claiming with a lease and `FOR UPDATE SKIP LOCKED`, globally unique
idempotency, persisted exponential backoff, pause/resume, dead-letter, requeue,
and expired-lease recovery; and a `Worker` that dispatches through an injected
`JobDispatcher` trait (no Docker/registry/TLS is introduced), promotes readiness
only from platform probes, redacts its failure diagnostics, and drains
in-flight work on graceful shutdown; plus a `control-plane` executable that
loads credentials from process configuration, runs migrations, and starts
bounded workers. 187 tests pass (156 pure-core + 31 control-plane: 8 config in
`crates/labrys-control-plane/tests/config.rs`, 6 redaction in
`crates/labrys-control-plane/tests/redaction.rs`, and 17 PostgreSQL integration
in `crates/labrys-control-plane/tests/control_plane.rs` covering restart
durability, stale-generation conflict, environment isolation, transactional
event rollback, redacted credential-bearing provider failure, concurrent
audit-chain integrity, two-worker single claim, duplicate idempotency,
lease-expiry recovery, agent-claim-never-ready, retry→dead-letter→requeue,
pause/resume, graceful shutdown, evidence retention, usage totals, and an
executable migration+recovery smoke). Container execution, image registries,
domains/TLS, the public HTTP API, the CLI binary, and the dashboard remain
later changes. No CI, deployment, or production evidence exists yet. The
Phase 5–7 queue (5 changes) is authored and planning-only; the crate now holds
durable persistence and a recoverable worker but still no container execution or
product entry points. The gate declaration is structurally valid and declares
commands for all 8 checks, and a fresh `driftwatchdog gate` run now PASSES all
8 (the earlier security-adapter block is resolved: `cargo audit` exits 0). The
only audit exception is a documented, scoped ignore of RUSTSEC-2023-0071
(`.cargo/audit.toml`) for `rsa`, which is a non-compiled optional transitive
dependency of sqlx's never-activated MySQL driver (`cargo tree -i rsa` is empty;
the binary links only `sqlx-postgres`).

## Current spec

`current_spec: container-execution-and-preview-runtime`

The package is newly authored and remains planning-only. It has not been
implemented, verified, or archived. It is the next Phase 5 change in `ROADMAP.md`
order and turns the runtime and preview contracts into bounded Docker execution,
isolated workspaces, health-gated previews, logs, and cleanup on top of the now
durable control plane. The Phase 0–4 queue plus `control-plane-persistence-and-
worker` remain implemented, locally verified, and archived.

## Verification evidence

- `cli-dashboard-and-plugin-sdk` tasks.md — 5/5 checked from implementation
  evidence.
- `openspec/changes/archive/2026-09-20-cli-dashboard-and-plugin-sdk` — archived
  with specs promoted (`platform-surfaces: create`, +3); canonical spec at
  `openspec/specs/platform-surfaces/spec.md` (3 requirements).
- `node scripts/check-openspec-change-names.mjs` — PASS; all active names are valid.
- `openspec list` — PASS; 5 active Phase 5–7 changes remain (this change archived).
- `openspec validate --all --strict` — PASS; 17 items passed, 0 failed (12
  promoted specs including the new `spec/control-plane-persistence` and
  `spec/reconciliation-worker`, plus the 5 active Phase 5–7 changes).
- `git diff --check` — PASS; no whitespace errors reported.
- `cargo build --workspace` — PASS; `cargo test --workspace` — PASS (187/187:
  the 156 pure-core tests unchanged plus 31 control-plane tests — 8/8 in
  `crates/labrys-control-plane/tests/config.rs`, 6/6 in
  `crates/labrys-control-plane/tests/redaction.rs`, and 17/17 in
  `crates/labrys-control-plane/tests/control_plane.rs` against an isolated
  PostgreSQL 16 instance).
- PostgreSQL integration evidence: the control-plane tests require
  `LABRYS_DATABASE_URL`/`DATABASE_URL`; when unset they skip and only the pure
  unit/core tests run. They were run against a throwaway `postgres:16-alpine`
  container; this is infrastructure evidence, not production-deployment evidence.
- Executable smoke: `cargo run --bin control-plane` connects, applies migrations,
  starts bounded workers, and drains to a clean stop on SIGTERM.
- `cargo fmt --all --check` — PASS; `cargo clippy --workspace --all-targets -- -D warnings` — PASS.
- `cargo audit` — PASS (exit 0) with one documented, scoped ignore of
  RUSTSEC-2023-0071 in `.cargo/audit.toml` for the non-compiled `rsa`
  transitive dependency of sqlx's inactive MySQL driver.
- Local Gate (`driftwatchdog gate` in repo root) — PASS (8/8 checks:
  build, format, lint, openspec_change_names, openspec_strict_validation,
  repository_integrity, security, tests); the earlier security-adapter block is
  resolved.
- `control-plane-persistence-and-worker` tasks.md — 17/17 checked from
  implementation and integration evidence.
- `openspec/changes/archive/2026-09-20-control-plane-persistence-and-worker` —
  archived with specs promoted (`control-plane-persistence: create`, +3;
  `reconciliation-worker: create`, +3).
- Deferred to later changes and NOT introduced here: container execution, image
  registries, DNS/TLS issuance, external provider implementations, the public
  HTTP API, the `labrys` CLI binary, and the operator dashboard.
- Prior history: foundation/runtime archived with one non-blocking proposal
  warning (missing Why/What Changes headers). Gate `notes`/`BLOCKED` schema fix
  and `driftwatchdog init` recorded in an earlier handoff revision.

## Handoff lifecycle

Select one active change, set `current_spec`, implement and verify it, and
archive only after local Gate and strict validation pass. Each spec change ends
with exactly two commits: commit 1 holds the implementation, tests, archived
change, and promoted specs; commit 2 is the separate `HANDOFF.md` pointer
update to the next active change. Stop after commit 2. If no active changes
remain, remove the pointer.
