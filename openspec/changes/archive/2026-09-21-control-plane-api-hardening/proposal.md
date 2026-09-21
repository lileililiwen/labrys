# Proposal: Control-plane API hardening

## Why

The Axum API trusts a single static bearer token (`LABRYS_API_TOKEN`,
minimum 16 characters in `crates/labrys-control-plane/src/api.rs`), binds
plain HTTP with no TLS story, and takes the actor from the
`x-labryst-actor` header after token check — so token rotation, scoped
credentials, transport confidentiality, and actor-identity binding are
all unproven. This is the internet-facing seam of every lifecycle
mutation and must harden before any staged or production exposure.

## What Changes

- Support multiple bearer tokens with metadata (identity, scope,
  expiry), rotation without restart, and revocation that takes effect on
  the next request; keep the single-token env form as a bootstrap that
  warns when used.
- Bind the request actor to the authenticated token identity
  (header actor must match token scope or the request is denied),
  preserving agent/human approval boundaries and attribution.
- Add TLS termination configuration (cert/key from files, plain HTTP
  only for loopback/disposable environments) plus request rate limits
  and audit logging of authentication failures without secret material.

## BFS Impact Map

- **Affected:** API auth middleware, token configuration, actor context,
  CLI token handling, API/CLI auth tests, staging smoke auth stages,
  release auth docs, dashboard API-client auth errors.
- **Dependencies:** none (parallelizable); dispatch wiring and provider
  adapters reuse the hardened gate without change.
- **Security:** tokens from process configuration only, constant-time
  comparison, failure responses without identity oracle, redacted audit
  of auth failures, TLS keys never logged.
- **Unaffected:** approval policy semantics, idempotency, job dispatch,
  secret cipher, provider adapters, dashboard views.

## Capabilities

- `control-plane-api-hardening`

## Non-goals

- Do not build full OIDC/OAuth or multi-tenant RBAC; token scopes stay
  agent/human with per-token environment scoping.
- Do not terminate mutual TLS or device attestation.
- Do not change approval, idempotency, or attribution event shapes.
