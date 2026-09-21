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
`openspec/specs/reconciliation-worker/spec.md` (3 requirements), and
`container-execution-and-preview-runtime` is implemented, locally verified,
and ARCHIVED as
`openspec/changes/archive/2026-09-21-container-execution-and-preview-runtime`,
with canonical specs promoted to
`openspec/specs/container-runtime-execution/spec.md` (2 requirements) and
`openspec/specs/preview-execution/spec.md` (1 requirement), and
`provider-registry-and-domain-adapters` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-21-provider-registry-and-domain-adapters`,
with canonical specs promoted to
`openspec/specs/provider-adapters/spec.md` (2 requirements) and
`openspec/specs/registry-and-domain-delivery/spec.md` (2 requirements), and
`control-plane-api-and-cli` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-21-control-plane-api-and-cli`, with canonical
specs promoted to `openspec/specs/control-plane-api/spec.md` (2 requirements)
and `openspec/specs/labrys-cli/spec.md` (2 requirements), and
`dashboard-and-operator-console` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-21-dashboard-and-operator-console`, with
canonical spec promoted to `openspec/specs/operator-dashboard/spec.md` (3
requirements), and `ci-packaging-and-release-evidence` is implemented,
locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-21-ci-packaging-and-release-evidence`,
with canonical specs promoted to
`openspec/specs/continuous-integration-and-gates/spec.md` (2 requirements)
and `openspec/specs/release-packaging-and-evidence/spec.md` (2 requirements).
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
as references only. A new `labrys-control-plane` execution layer (`crates/labrys-control-plane/src/runtime.rs`,
`crates/labrys-control-plane/src/preview.rs`, migration
`20260921000001_execution_and_previews.sql`) turns the runtime and preview
contracts into bounded Docker execution without making the pure core depend on
I/O: an injected `ContainerExecutor` trait with a real `DockerExecutor`
(structured build/run/stop/inspect/log/health arguments, timeouts,
cancellation, redacted diagnostics, no shell on path) and an
`UnavailableExecutor` that reports a missing runtime as an environment
blocker, never a simulated pass; `enforce_limits_before_schedule` plus
`docker_run_args`/`EffectiveLimits` recording the effective CPU, memory,
filesystem, network, capability, and PID posture; `probe_health` platform-only
health semantics and `verify_production_artifact` refusing to promote a
development process; `prospective_phase` mapping platform health to
`ResourcePhase`; `WorkspaceRoot` isolated session workspaces,
`PreviewManager` health-gated startup with expiry/revocation/cleanup and Expo
transport metadata, and `PgExecutionStore`/`PgPreviewStore` persisting
execution rows, preview state, platform-actor events, separated runtime logs,
and health/build-test evidence with secret redaction. 216 tests pass (156
pure-core + 60 control-plane: 12 lib + 8 config + 17 container-execution — 10
pure unit, 3 PostgreSQL-backed, 4 Docker-backed — plus the prior 17
control-plane and 6 redaction suites) against an isolated PostgreSQL 16
instance and a real Docker 29.5.3 daemon. The `labrys-control-plane` crate
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
bounded workers. 216 tests pass (156 pure-core + 60 control-plane: 12 lib,
8 config in `crates/labrys-control-plane/tests/config.rs`, 6 redaction in
`crates/labrys-control-plane/tests/redaction.rs`, 17 PostgreSQL integration
in `crates/labrys-control-plane/tests/control_plane.rs` covering restart
durability, stale-generation conflict, environment isolation, transactional
event rollback, redacted credential-bearing provider failure, concurrent
audit-chain integrity, two-worker single claim, duplicate idempotency,
lease-expiry recovery, agent-claim-never-ready, retry→dead-letter→requeue,
pause/resume, graceful shutdown, evidence retention, usage totals, and an
executable migration+recovery smoke, plus 17 container-execution in
`crates/labrys-control-plane/tests/container_execution.rs` covering
unsupported-runtime rejection, limit enforcement, argument structure, health
semantics, workspace isolation, revocation, redaction, persisted execution and
preview rows, concurrent sessions, build timeout cancellation, unhealthy
containers yielding no URL, and expiry revocation against an isolated
PostgreSQL 16 instance and a real Docker daemon), plus 11 provider-delivery
in `crates/labrys-core/tests/provider_delivery.rs` and 10 provider-delivery
in `crates/labrys-control-plane/tests/provider_delivery.rs` covering scoped
provisioning across all five provider kinds, accepted-not-ready semantics,
credential redaction with recovery, approval-denied deletion with zero
provider calls, digest-mismatch refusal, certificate-failure reporting, and
durable operation/evidence persistence against an isolated PostgreSQL 16
instance, plus 12 API/CLI integration tests in
`crates/labrys-control-plane/tests/api_cli.rs` covering version negotiation,
auth, import→inspect→doctor with durable replay, agent deploy denial with
zero jobs, deploy replay with one job, in-progress-never-healthy health,
rollback boundaries and the database-data warning, log redaction and
pagination, plugin accept/refuse, CLI black-box doctor and agent rollback,
and attributed correlated events against an isolated PostgreSQL 16 instance,
a real HTTP server, and a real `labrys` subprocess. The dashboard remains a
later change. No CI, deployment, or production evidence exists yet. The
Phase 5–7 queue (2 active changes) is authored and planning-only; the crate
now holds durable persistence, a recoverable worker, bounded
container/preview execution, provider/registry/domain delivery adapters, and
an authenticated API/CLI entry point. The gate declaration is structurally valid and declares
commands for all 8 checks, and a fresh `driftwatchdog gate` run now PASSES all
8 (the earlier security-adapter block is resolved: `cargo audit` exits 0). The
only audit exception is a documented, scoped ignore of RUSTSEC-2023-0071
(`.cargo/audit.toml`) for `rsa`, which is a non-compiled optional transitive
dependency of sqlx's never-activated MySQL driver (`cargo tree -i rsa` is empty;
the binary links only `sqlx-postgres`).

## Current spec

Four planning-only changes remain authored (not implemented); all tasks are
unchecked planning artifacts:

- `executable-dispatch-wiring` is implemented, locally verified, and ARCHIVED
  as `openspec/changes/archive/2026-09-21-executable-dispatch-wiring`, with
  the canonical spec promoted to
  `openspec/specs/executable-dispatch-wiring/spec.md` (2 requirements).
- `secret-encryption-hardening` (parallelizable, no dependencies)
- `control-plane-api-hardening` (parallelizable, no dependencies)
- `runtime-tier-depth-expansion` (parallelizable, no dependencies)
- `real-provider-delivery-adapters` (depended on
  `executable-dispatch-wiring`, now unblocked) — `current_spec:
  real-provider-delivery-adapters`

The pointer marks the next dependency-ready change; its dependency just
landed. Implementation of it has not started.

Verification evidence for `executable-dispatch-wiring` (commit `41f7e0d`):
`cargo test --workspace` 268/268 against isolated PostgreSQL 16 + Docker,
smoke-staging 17/17 with `runtime-delivery` passed for real,
`collect-evidence.sh` 14 checks 0 failed (new `staging-execution` check),
`driftwatchdog gate` 8/8, `openspec validate --all --strict` 26 items
passed. One pre-existing latent bug fixed as a verification prerequisite:
`scripts/secret-scan.sh` flagged its own `sk-live-` pattern line; the
alternative is now spelled `sk-liv[e]-` (detection unchanged).

`ci-packaging-and-release-evidence` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-21-ci-packaging-and-release-evidence`,
with canonical specs promoted to
`openspec/specs/continuous-integration-and-gates/spec.md` (2 requirements)
and `openspec/specs/release-packaging-and-evidence/spec.md` (2
requirements). CI (`.github/workflows/ci.yml`) runs dependency-ordered
static, rust+PostgreSQL-service, dashboard-build, security, and
package-and-evidence jobs; `compose.test.yml` gives a disposable local
postgres:16; `scripts/` holds the secret scan, migration check (6/6 applied,
schema 20260922000001), versioned packaging (binaries, dashboard bundle,
SHA256SUMS, release manifest, lockfile SBOM, cosign-or-UNSIGNED), staging
smoke (10/10 stages; runtime-delivery honestly blocked on the no-op
dispatcher), and tiered evidence collection (13 checks, 0 failed;
production-proof always blocked). `docs/release.md` defines the evidence
vocabulary and release/rollback/incident/blocked procedures.
`cargo test --workspace` passes (249/249, integration enabled) and
`driftwatchdog gate` passes 8/8. All roadmap phases are implemented,
locally verified, and archived; resuming work means authoring a new change
(or revising the roadmap).

`dashboard-and-operator-console` is implemented, locally verified, and
ARCHIVED as
`openspec/changes/archive/2026-09-21-dashboard-and-operator-console`, with
canonical spec promoted to `openspec/specs/operator-dashboard/spec.md` (3
requirements). The `dashboard/` Next.js operator console
(`src/lib/api.ts` typed v1 client, `src/lib/attribution.ts` platform-wins
health, `src/lib/rollback.ts` database-data-warning gate,
`src/lib/secrets.ts` reference-only boundary, `src/components/` accessible
views, `src/app/` routes) distinguishes platform evidence from agent claims,
blocks rollback until warning acknowledgement plus scope confirmation plus
named approver, and never renders secret values. `npm run typecheck` passes,
19/19 vitest tests pass, `npm run build` prerenders the production bundle,
`cargo test --workspace` passes (249/249), and `driftwatchdog gate` passes
8/8.

## Verification evidence

- `ci-packaging-and-release-evidence` tasks.md — 12/12 checked from
  implementation and integration evidence.
- `openspec/changes/archive/2026-09-21-ci-packaging-and-release-evidence` —
  archived with specs promoted (`continuous-integration-and-gates: create`,
  +2; `release-packaging-and-evidence: create`, +2); canonical specs at
  `openspec/specs/continuous-integration-and-gates/spec.md` and
  `openspec/specs/release-packaging-and-evidence/spec.md`.
- `bash scripts/secret-scan.sh` — PASS (only allowlisted fake fixtures).
- `bash scripts/verify-migrations.sh` — PASS (6/6 applied, schema version
  20260922000001) against an isolated scratch database.
- `bash scripts/package.sh --out` — PASS (release binaries, dashboard
  bundle, 83 files checksummed, release manifest with source rev / protocol
  / migration / tier scope, 267-package lockfile SBOM, UNSIGNED marker in
  place of unavailable cosign).
- `bash scripts/smoke-staging.sh --out` — PASS (10/10 stages: version,
  import, inspect, health-never-healthy, doctor, agent-deploy-denied,
  rollback-without-approval-denied, logs, negotiate, daemon-present;
  runtime-delivery recorded blocked, never passed).
- `bash scripts/collect-evidence.sh --out` — PASS (13 checks, 0 failed, 0
  blocked locally; production-proof recorded blocked by policy).
- `node scripts/check-openspec-change-names.mjs` — PASS; `openspec list` —
  PASS (no active changes remain).
- `openspec validate --all --strict` — PASS (21 items: 21 promoted specs).
- `git diff --check` — PASS; `cargo fmt --all --check` — PASS;
  `cargo clippy --workspace --all-targets -- -D warnings` — PASS.
- `cargo build --workspace` — PASS; `cargo test --workspace` — PASS
  (249/249 with PostgreSQL integration enabled against an isolated
  PostgreSQL 16 instance and a real Docker daemon).
- `cargo audit` — PASS (exit 0).
- Local Gate (`driftwatchdog gate` in repo root) — PASS (8/8 checks).

`control-plane-api-and-cli` is implemented, locally verified, and ARCHIVED as
`openspec/changes/archive/2026-09-21-control-plane-api-and-cli`, with canonical
specs promoted to `openspec/specs/control-plane-api/spec.md` (2 requirements)
and `openspec/specs/labrys-cli/spec.md` (2 requirements). The
`labrys-control-plane` crate now serves an authenticated Axum API
(`crates/labrys-control-plane/src/api.rs`, 17 routes): bearer-token auth with
actor/trace/idempotency headers, agent-denied human-only mutations, named
approval for rollback, durable `idempotency_records` replay (migration
`20260922000001_api_idempotency.sql`), accepted/job-reference mutation
envelopes that never report healthy without platform observation, redacted
paginated reads, attributable correlated events, agent/plugin protocol
negotiation with 409 on mismatch, and `latest_for_target` job lookup for
in-progress health. The distributable `labrys` CLI
(`crates/labrys-control-plane/src/bin/labrys.rs` via clap plus the typed
`ControlPlaneClient`) covers all 15 stable lifecycle commands with
`init`/`dev` aliases, pretty JSON envelopes, recovery on stderr, and exit
0/1/2 semantics.

## Verification evidence

- `dashboard-and-operator-console` tasks.md — 12/12 checked from
  implementation and integration evidence.
- `openspec/changes/archive/2026-09-21-dashboard-and-operator-console` —
  archived with specs promoted (`operator-dashboard: create`, +3); canonical
  spec at `openspec/specs/operator-dashboard/spec.md` (3 requirements).
- `npm run typecheck` — PASS; `npm test` — PASS (19/19 vitest: attribution,
  rollback approval boundary, secret references, dashboard view platform
  truth, typed API-client contract).
- `npm run build` — PASS; production bundle prerenders (`/` 1.07 kB, first
  load 88.3 kB) with platform-failure-wins attribution, rollback warning
  gate, and reference-only secrets.
- `node scripts/check-openspec-change-names.mjs` — PASS; all active names are valid.
- `openspec list` — PASS; 1 active Phase 7 change remains (this change archived).
- `openspec validate --all --strict` — PASS; 20 items passed, 0 failed (19
  promoted specs including the new `spec/operator-dashboard`, plus the 1
  active Phase 7 change).
- `git diff --check` — PASS; no whitespace errors reported.
- `cargo fmt --all --check` — PASS; `cargo clippy --workspace --all-targets -- -D warnings` — PASS.
- `cargo build --workspace` — PASS; `cargo test --workspace` — PASS
  (249/249, 0 failed).
- `cargo audit` — PASS (exit 0). `npm audit` notes scoped, non-blocking
  build/dev-time findings (esbuild dev-server, next/postcss server paths
  unused by the static bundle; Next.js kept at latest patched 14.x); see
  `dashboard/README.md`.
- Local Gate (`driftwatchdog gate` in repo root) — PASS (8/8 checks:
  build, format, lint, openspec_change_names, openspec_strict_validation,
  repository_integrity, security, tests).

- `container-execution-and-preview-runtime` tasks.md — 13/13 checked from
  implementation and integration evidence.
- `openspec/changes/archive/2026-09-21-container-execution-and-preview-runtime` —
  archived with specs promoted (`container-runtime-execution: create`, +2;
  `preview-execution: create`, +1); canonical specs at
  `openspec/specs/container-runtime-execution/spec.md` (2 requirements) and
  `openspec/specs/preview-execution/spec.md` (1 requirement).
- `node scripts/check-openspec-change-names.mjs` — PASS; all active names are valid.
- `openspec list` — PASS; 4 active Phase 5–7 changes remain (this change archived).
- `openspec validate --all --strict` — PASS; 18 items passed, 0 failed (14
  promoted specs including the new `spec/container-runtime-execution` and
  `spec/preview-execution`, plus the 4 active Phase 5–7 changes).
- `git diff --check` — PASS; no whitespace errors reported.
- `cargo build --workspace` — PASS; `cargo test --workspace` — PASS (216/216:
  the 156 pure-core tests unchanged plus 60 control-plane tests — 12/12 lib,
  8/8 in `crates/labrys-control-plane/tests/config.rs`, 6/6 in
  `crates/labrys-control-plane/tests/redaction.rs`, 17/17 in
  `crates/labrys-control-plane/tests/control_plane.rs`, and 17/17 in
  `crates/labrys-control-plane/tests/container_execution.rs` against an
  isolated PostgreSQL 16 instance and a real Docker 29.5.3 daemon).
- PostgreSQL/Docker integration evidence: the control-plane and
  container-execution tests require `LABRYS_DATABASE_URL`/`DATABASE_URL` (and a
  Docker daemon for the 4 Docker-backed tests); when unset they skip and only
  the pure unit/core tests run. They were run against a throwaway
  `postgres:16-alpine` container plus the local Docker daemon; this is
  infrastructure evidence, not production-deployment evidence.
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
- Deferred to later changes and NOT introduced here: image registries,
  DNS/TLS issuance, external provider implementations, the public HTTP API,
  the `labrys` CLI binary, and the operator dashboard.
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
