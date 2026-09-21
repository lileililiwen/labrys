# Design

Auth resolves each request to a token record (id, identity, scope,
expiry) from an in-memory set hot-reloaded from process configuration;
the bootstrap single token maps to a human-scoped record and emits a
startup warning directing operators to rotation. `actor_context` derives
the actor from the token record and rejects header actors outside the
token's scope, so a scoped agent token can never claim human identity.
Revoked or expired tokens fail closed with the existing 401 recovery
payload.

The server binds TLS when cert/key paths are configured (rustls) and
refuses non-loopback plain HTTP unless explicitly flagged for
disposable environments. A bounded rate limiter sits in front of auth
to blunt credential probing, and auth failures emit redacted audit
events (token id prefix only, never values). CLI gains `--token` plus
env handling unchanged, with clearer recovery on 401/403.
