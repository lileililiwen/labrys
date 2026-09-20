# Labrys Governance and OpenSpec Planning Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish durable Labrys product governance and a dependency-ordered OpenSpec queue covering the complete supplied platform brief.

**Architecture:** Keep Labrys centered on `Application`, with pluggable `AgentBackend`, language-independent `RuntimeAdapter`, and explicit `Capability`, `Provider`, `Binding`, `Resource`, `Inspector`, `Controller`, and `Verifier` boundaries. Use OpenSpec changes as delegated implementation handoffs; this bootstrap creates no runtime implementation.

**Tech Stack:** Planned control plane: Rust, Tokio, Axum, SQLx, Serde, Tracing, OpenTelemetry. Planned dashboard: Next.js, React, TypeScript, Tailwind CSS, shadcn/ui. OpenSpec schema: `spec-driven`.

**Spec:** `ROADMAP.md` and the ten active changes under `openspec/changes/`.

## Global Constraints

- The top-level domain object is `Application`, not chat or agent session.
- Existing projects and generated projects use the same application model.
- Container compatibility is the minimum runtime boundary.
- Platform API actions precede framework tools, generic tools, and raw shell.
- Secrets never enter LLM context; runtimes receive injected references.
- Production destructive actions require human approval by default.
- Planning artifacts are not implementation or runtime evidence.

### Task 1: Bootstrap repository governance

**Files:** `README.md`, `ROADMAP.md`, `HANDOFF.md`, `AGENTS.md`, `.ai-rules/*`, `.ai-gate/gate.yaml`, `.gitignore`, `.agentignore`, `openspec/config.yaml`.

- [ ] Record product definition, architecture invariants, MVP/deferred scope, and evidence boundaries.
- [ ] Define BFS → DFS → BFS workflow, current-spec lifecycle, completion rules, and local gate declaration.
- [ ] Set the first active OpenSpec pointer to `application-foundation-and-desired-state`.

### Task 2: Author the dependency-ordered OpenSpec queue

**Files:** `openspec/changes/<change>/{proposal.md,design.md,tasks.md,specs/<capability>/spec.md}`.

- [ ] Create one change for each major platform boundary in `ROADMAP.md` order.
- [ ] Include observable success, failure, safety, and boundary scenarios in every capability spec.
- [ ] Keep all names lowercase, letter-first kebab-case and validate them before status/instructions use.

### Task 3: Validate documentation and planning artifacts

**Files:** `scripts/check-openspec-change-names.mjs`.

- [ ] Run the local change-name checker.
- [ ] Run strict OpenSpec validation.
- [ ] Run repository diff checks and inspect Git status for intended files only.
- [ ] Report the repository as planning/governance initialized, not runtime-complete.
