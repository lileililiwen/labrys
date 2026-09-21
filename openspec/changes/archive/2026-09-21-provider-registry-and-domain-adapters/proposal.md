# Proposal: Provider, registry, and domain delivery adapters

## Why

The core models providers, OCI images, domains, and capabilities but performs
no external operation. Registry pushes, capability provisioning, secret
injection, DNS/TLS issuance, and traffic attachment remain unimplemented.

## What Changes

- Add process-safe provider adapters for PostgreSQL, auth, file/object storage,
  and generic capability operations.
- Add digest-verified OCI registry push/pull and artifact lifecycle handling.
- Add approval-gated domain, DNS, TLS, and traffic attachment adapters.
- Connect provider observations and failures to controllers, events, health,
  and rollback records.

## BFS Impact Map

- **Dependencies:** persistence/worker and container execution.
- **Affected:** capability bindings, secret references, deployment artifacts,
  domains, health, audit, approvals, and rollback.
- **Security:** credentials remain references, provider calls are scoped,
  destructive operations require named human approval, and external errors are
  redacted.
- **Unaffected:** public API, dashboard, and cloud-specific marketplace.

## Capabilities

- `capability-provider-adapters`
- `registry-and-domain-delivery`

## Non-goals

- Do not implement Kubernetes or broad cloud marketplace support.
- Do not auto-adopt or delete existing external resources.
