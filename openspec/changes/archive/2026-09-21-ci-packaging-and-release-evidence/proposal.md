# Proposal: CI, packaging, and release evidence

## Why

The repository has no CI, executable packaging, deployment manifests, or
runtime/release evidence. Local model tests and strict OpenSpec validation do
not prove a usable Labrys product.

## What Changes

- Add CI for Rust, SQLx migrations, PostgreSQL integration, container runtime,
  API/CLI, dashboard, security, and OpenSpec gates.
- Produce versioned control-plane, CLI, plugin SDK, and dashboard artifacts.
- Add reproducible local Compose/test environments, release checksums,
  migration checks, SBOM/signing hooks, and evidence collection.
- Define staging smoke tests and explicit evidence boundaries for supported
  runtime/provider tiers.

## BFS Impact Map

- **Dependencies:** all implementation packages in Phases 5–9.
- **Affected:** manifests, CI, packaging, migrations, test services, release
  metadata, security scans, runtime smoke tests, and documentation.
- **Unaffected:** deferred Kubernetes, GPU cloud, marketplace, and complex
  billing scope.
- **Evidence:** separate static/model, integration, staging, and production
  claims; unavailable infrastructure remains blocked rather than passed.

## Capabilities

- `continuous-integration-and-gates`
- `release-packaging-and-evidence`

## Non-goals

- Do not claim production readiness from CI alone.
- Do not add cloud deployment automation without explicit provider scope.
