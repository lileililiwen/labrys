# Proposal: Real provider delivery adapters

## Why

Capability provisioning, OCI registry delivery, and DNS/TLS/domain traffic
run only through local test doubles (`LocalTestAdapter`,
`LocalTestRegistry`, `LocalDomainDelivery` in
`crates/labrys-control-plane/src/providers.rs`). `docs/release.md`
explicitly scopes providers to test doubles and states cloud deployment
automation is out of scope until an explicit provider change lands, so no
real database, object-storage, registry-push, or domain-issuance path is
executable.

## What Changes

- Add real adapters behind the existing `ProviderAdapter`, `OciRegistry`,
  and `DomainDeliveryAdapter` traits: managed PostgreSQL provisioning,
  object-storage/file-storage provisioning, OCI registry push with
  digest verification, and DNS/TLS issuance plus approval-gated traffic
  attachment.
- Source all provider credentials from process configuration and secret
  references (never agent context), redact them before persistence, and
  keep destructive actions denied without named approval.
- Persist provider operations, observations, and digest/certificate
  evidence through `ProviderRuntime` and extend provider-delivery
  integration tests to the real adapters against disposable Espanha.

## BFS Impact Map

- **Affected:** provider adapters, registry delivery, domain delivery,
  provider runtime persistence, worker dispatch for provider jobs, secrets
  injection scope, provider-delivery tests, release supported scope.
- **Dependencies:** `executable-dispatch-wiring` (real dispatch path);
  independent of API hardening and secret cipher work.
- **Security:** credentials from environment/secret refs only, redacted
  before SQL, least-privilege provider tokens, approval-gated deletion
  and traffic attachment, digest-mismatch refusal.
- **Unaffected:** runtime container execution, preview lifecycle, API
  auth model, dashboard views, Tier 3 framework depth.

## Capabilities

- `real-provider-delivery`

## Non-goals

- Do not add Kubernetes, GPU scheduling, marketplace, billing, or
  app-store automation.
- Do not change the `ProviderAdapter`/`OciRegistry`/`DomainDeliveryAdapter`
  trait shapes unless a missing operation is proven; extend, don't churn.
- Do not store provider credentials in the database or logs in any form.
