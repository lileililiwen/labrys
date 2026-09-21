//! Hardened control-plane API authentication.
//!
//! Covers `control-plane-api-hardening`: the API authenticates against
//! multiple scoped tokens with expiry, supports rotation without restart,
//! and enforces revocation on the next request. The single-token
//! `LABRYS_API_TOKEN` form stays as a bootstrap that warns at startup.
//! Request actors bind to the token scope (an agent-scoped token can never
//! claim a human actor and vice versa), responses never distinguish
//! unknown/expired/revoked tokens (no identity oracle), and every denial
//! is audit-logged with token ids or hash fingerprints — never values.
//!
//! Tokens arrive from process configuration only (inline
//! `LABRYS_API_TOKENS`, `LABRYS_API_TOKENS_FILE`, or bootstrap
//! `LABRYS_API_TOKEN`); raw secrets live only in [`TokenStore`] memory,
//! are zeroized on drop, and never enter logs, events, or audit rows.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::error::{ControlPlaneError, Result};

/// Minimum bearer secret length, matching the bootstrap rule.
pub const MIN_TOKEN_LEN: usize = 16;

/// Default per-IP request budget per minute when unconfigured.
pub const DEFAULT_RATE_LIMIT_PER_MINUTE: u32 = 120;

/// Token line format: `<id>:<scope>:<expiry>:<secret>`, one per line.
/// Scope is `agent` or `human`; expiry is RFC 3339 or `never`. The id,
/// scope, and expiry never contain `:`; the secret runs to end of line
/// after the last `:` (so secrets must not contain `:` either — generate
/// alphanumeric tokens).
pub const TOKEN_LINE_HELP: &str =
    "one '<id>:<scope>:<expiry>:<secret>' line per entry (scope agent|human, expiry RFC3339|never, no ':' in id or secret)";

/// Credential scope bound to every token. Agent/human approval boundaries
/// and attribution shapes are unchanged; the scope only decides which
/// header actor a token may present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    Human,
    Agent,
}

impl TokenScope {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "human" => Some(Self::Human),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    /// Human tokens present `human:*` actors; agent tokens present
    /// `agent:*` actors. Platform-owned `platform:*` headers are never
    /// accepted over HTTP: platform attribution stays server-side.
    pub fn allows(&self, actor: &labrys_core::CliActor) -> bool {
        matches!(
            (self, actor),
            (Self::Human, labrys_core::CliActor::Human { .. })
                | (Self::Agent, labrys_core::CliActor::Agent { .. })
        )
    }
}

/// One bearer credential: identity metadata plus the secret. The secret is
/// zeroized on drop and never rendered by `Debug`.
pub struct TokenRecord {
    /// Stable operator-chosen id used in audit rows (never the secret).
    pub id: String,
    /// Human-readable owner, e.g. `ci-deployer` or `agent-runner`.
    pub identity: String,
    pub scope: TokenScope,
    /// `None` means no expiry.
    pub expires_at: Option<DateTime<Utc>>,
    /// Set by [`TokenStore::revoke`]; revoked tokens fail closed with
    /// their id preserved for audit.
    pub revoked: bool,
    secret: Vec<u8>,
}

impl Clone for TokenRecord {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            identity: self.identity.clone(),
            scope: self.scope,
            expires_at: self.expires_at,
            revoked: self.revoked,
            secret: self.secret.clone(),
        }
    }
}

impl std::fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("id", &self.id)
            .field("identity", &self.identity)
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .field("secret", &"[redacted]")
            .finish()
    }
}

impl Drop for TokenRecord {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl TokenRecord {
    fn new(
        id: String,
        identity: String,
        scope: TokenScope,
        expires_at: Option<DateTime<Utc>>,
        secret: Vec<u8>,
    ) -> Result<Self> {
        if id.trim().is_empty() {
            return Err(ControlPlaneError::Config(
                "API token id must not be empty".to_string(),
            ));
        }
        if secret.len() < MIN_TOKEN_LEN {
            return Err(ControlPlaneError::Config(format!(
                "API token '{id}' must be at least {MIN_TOKEN_LEN} characters"
            )));
        }
        Ok(Self {
            id,
            identity,
            scope,
            expires_at,
            revoked: false,
            secret,
        })
    }

    fn digest(&self) -> [u8; 32] {
        digest_secret(&self.secret)
    }
}

fn digest_secret(secret: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(secret);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Fingerprint recorded for *unknown* presented tokens: a truncated hash,
/// safe to persist because it reveals nothing about the secret value.
pub fn token_fingerprint(presented: &str) -> String {
    let digest = digest_secret(presented.as_bytes());
    format!("sha256:{}", hex_prefix(&digest, 8))
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    bytes
        .iter()
        .take(len)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// Why authentication failed. Responses never distinguish these (no
/// identity oracle); the distinction exists only for redacted audit rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDenyReason {
    Unknown,
    Expired,
    Revoked,
}

/// Authenticated identity handed to actor binding: id, owner, and scope.
/// No secret material leaves [`TokenStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedToken {
    pub id: String,
    pub identity: String,
    pub scope: TokenScope,
}

/// A denial carrying the known token id when the token exists (expired or
/// revoked) so audit rows stay precise without secret material.
#[derive(Debug, Clone)]
pub struct AuthDenial {
    pub reason: AuthDenyReason,
    pub known_id: Option<String>,
}

/// In-memory token set. Every mutation (reload, revoke) swaps under a
/// lock, so rotation and revocation take effect on the next request
/// without restarting the process.
#[derive(Debug)]
pub struct TokenStore {
    tokens: RwLock<Vec<TokenRecord>>,
    bootstrap: bool,
}

impl TokenStore {
    /// Single-token bootstrap from `LABRYS_API_TOKEN`: a human-scoped
    /// record that warns at startup directing operators to rotation.
    pub fn bootstrap(secret: String) -> Result<Arc<Self>> {
        let record = TokenRecord::new(
            "bootstrap".to_string(),
            "bootstrap-operator".to_string(),
            TokenScope::Human,
            None,
            secret.into_bytes(),
        )?;
        Ok(Arc::new(Self {
            tokens: RwLock::new(vec![record]),
            bootstrap: true,
        }))
    }

    /// Loads from process configuration: `LABRYS_API_TOKENS_FILE` wins over
    /// inline `LABRYS_API_TOKENS`, falling back to the bootstrap token.
    pub fn from_env() -> Result<Arc<Self>> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Arc<Self>> {
        if let Some(path) = lookup("LABRYS_API_TOKENS_FILE") {
            let raw = std::fs::read_to_string(path.trim()).map_err(|e| {
                ControlPlaneError::Config(format!(
                    "cannot read LABRYS_API_TOKENS_FILE: {e}; check the path and permissions, then retry"
                ))
            })?;
            return Self::from_lines(&raw);
        }
        if let Some(raw) = lookup("LABRYS_API_TOKENS") {
            return Self::from_lines(&raw);
        }
        let secret = lookup("LABRYS_API_TOKEN").unwrap_or_default();
        if secret.trim().is_empty() {
            return Err(ControlPlaneError::Config(
                "API token is required (LABRYS_API_TOKEN)".to_string(),
            ));
        }
        Self::bootstrap(secret)
    }

    /// Parses `TOKEN_LINE_HELP` lines; blank lines are ignored.
    pub fn from_lines(raw: &str) -> Result<Arc<Self>> {
        let mut records = Vec::new();
        for (index, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            records.push(parse_token_line(line).map_err(|e| {
                ControlPlaneError::Config(format!("token entry {}: {e}", index + 1))
            })?);
        }
        if records.is_empty() {
            return Err(ControlPlaneError::Config(
                "no API tokens configured; provide LABRYS_API_TOKENS(_FILE) or LABRYS_API_TOKEN"
                    .to_string(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for record in &records {
            if !ids.insert(record.id.clone()) {
                return Err(ControlPlaneError::Config(format!(
                    "duplicate API token id '{}'",
                    record.id
                )));
            }
        }
        Ok(Arc::new(Self {
            tokens: RwLock::new(records),
            bootstrap: false,
        }))
    }

    /// Re-reads process configuration and swaps the set: rotation without
    /// restart. Returns the loaded record count. A failed reload keeps the
    /// live set untouched; the replaced set (and its secrets) is dropped —
    /// and zeroized — on swap.
    pub fn reload(&self, lookup: impl Fn(&str) -> Option<String>) -> Result<usize> {
        let fresh = Self::from_lookup(lookup)?;
        let staged: Vec<TokenRecord> = fresh.tokens.read().expect("tokens lock").clone();
        let count = staged.len();
        *self.tokens.write().expect("tokens lock") = staged;
        Ok(count)
    }

    /// Revokes a token by id, preserving the record so the next request
    /// bearing it fails closed with its id in the audit row. Returns true
    /// when a record was marked.
    pub fn revoke(&self, id: &str) -> bool {
        let mut guard = self.tokens.write().expect("tokens lock");
        let mut marked = false;
        for record in guard.iter_mut() {
            if record.id == id && !record.revoked {
                record.revoked = true;
                marked = true;
            }
        }
        marked
    }

    /// Constant-time authentication against the live set. Expired tokens
    /// fail closed; unknown secrets yield no id (fingerprint instead).
    pub fn authenticate(
        &self,
        presented: &str,
    ) -> std::result::Result<AuthenticatedToken, AuthDenial> {
        let digest = digest_secret(presented.as_bytes());
        let guard = self.tokens.read().expect("tokens lock");
        let mut matched: Option<&TokenRecord> = None;
        for record in guard.iter() {
            if bool::from(record.digest().as_slice().ct_eq(digest.as_slice())) {
                matched = Some(record);
                break;
            }
        }
        let Some(record) = matched else {
            return Err(AuthDenial {
                reason: AuthDenyReason::Unknown,
                known_id: None,
            });
        };
        if record.revoked {
            return Err(AuthDenial {
                reason: AuthDenyReason::Revoked,
                known_id: Some(record.id.clone()),
            });
        }
        if let Some(expires) = record.expires_at {
            if Utc::now() >= expires {
                return Err(AuthDenial {
                    reason: AuthDenyReason::Expired,
                    known_id: Some(record.id.clone()),
                });
            }
        }
        Ok(AuthenticatedToken {
            id: record.id.clone(),
            identity: record.identity.clone(),
            scope: record.scope,
        })
    }

    pub fn len(&self) -> usize {
        self.tokens.read().expect("tokens lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// True when this store is the single-token bootstrap: callers print
    /// [`Self::bootstrap_warning`] once at startup.
    pub fn is_bootstrap(&self) -> bool {
        self.bootstrap
    }

    pub fn bootstrap_warning(&self) -> Option<String> {
        self.bootstrap.then(|| {
            "API auth uses the single-token LABRYS_API_TOKEN bootstrap; configure LABRYS_API_TOKENS(_FILE) with scoped tokens and rotate without restart".to_string()
        })
    }

    /// Token ids for operator diagnostics (never secrets).
    pub fn token_ids(&self) -> Vec<String> {
        self.tokens
            .read()
            .expect("tokens lock")
            .iter()
            .map(|r| r.id.clone())
            .collect()
    }
}

fn parse_token_line(line: &str) -> std::result::Result<TokenRecord, String> {
    // The expiry (RFC 3339) contains ':' itself, so the id/scope split
    // from the left while expiry/secret split at the last ':'.
    let mut head = line.splitn(3, ':');
    let (Some(id), Some(scope), Some(rest)) = (head.next(), head.next(), head.next()) else {
        return Err(format!("expected {TOKEN_LINE_HELP}"));
    };
    let Some((expiry, secret)) = rest.rsplit_once(':') else {
        return Err(format!("expected {TOKEN_LINE_HELP}"));
    };
    let scope = TokenScope::parse(scope)
        .ok_or_else(|| format!("scope must be 'agent' or 'human'; expected {TOKEN_LINE_HELP}"))?;
    let expires_at = match expiry.trim() {
        "never" => None,
        raw => Some(raw.parse::<DateTime<Utc>>().map_err(|_| {
            format!("expiry must be RFC3339 or 'never'; got '{raw}' (note: secrets must not contain ':')")
        })?),
    };
    if secret.is_empty() {
        return Err(format!(
            "secret must not be empty; expected {TOKEN_LINE_HELP}"
        ));
    }
    TokenRecord::new(
        id.trim().to_string(),
        id.trim().to_string(),
        scope,
        expires_at,
        secret.to_string().into_bytes(),
    )
    .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// TLS termination policy
// ---------------------------------------------------------------------------

/// TLS termination files from process configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsConfig {
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
}

/// How the server may bind: loopback plain HTTP, TLS, or explicitly
/// flagged disposable plain HTTP. Non-loopback plain HTTP without the
/// explicit flag is refused at startup, never silently served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindPolicy {
    PlainLoopback,
    Tls,
    PlainDisposable,
}

pub fn bind_policy(
    bind_addr: &std::net::SocketAddr,
    tls: Option<&TlsConfig>,
    allow_plain_http: bool,
) -> Result<BindPolicy> {
    if let Some(tls) = tls {
        if !tls.cert_file.is_file() {
            return Err(ControlPlaneError::Config(format!(
                "TLS cert file '{}' is missing; check LABRYS_TLS_CERT_FILE, then retry",
                tls.cert_file.display()
            )));
        }
        if !tls.key_file.is_file() {
            return Err(ControlPlaneError::Config(format!(
                "TLS key file '{}' is missing; check LABRYS_TLS_KEY_FILE, then retry",
                tls.key_file.display()
            )));
        }
        return Ok(BindPolicy::Tls);
    }
    if bind_addr.ip().is_loopback() {
        return Ok(BindPolicy::PlainLoopback);
    }
    if allow_plain_http {
        return Ok(BindPolicy::PlainDisposable);
    }
    Err(ControlPlaneError::Config(format!(
        "refusing plain HTTP on non-loopback {bind_addr}; set LABRYS_TLS_CERT_FILE/LABRYS_TLS_KEY_FILE or LABRYS_ALLOW_PLAIN_HTTP=1 for disposable environments"
    )))
}

// ---------------------------------------------------------------------------
// Per-IP fixed-window rate limiter (sits in front of auth)
// ---------------------------------------------------------------------------

/// Bounded per-IP fixed-window limiter blunting credential probing.
/// `check` is timing-safe for tests: no sleeps, window arithmetic only.
#[derive(Debug)]
pub struct RateLimiter {
    limit_per_minute: u32,
    window: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute: limit_per_minute.max(1),
            window: Mutex::new(HashMap::new()),
        }
    }

    /// Records one request from `ip`. On budget exhaustion returns the
    /// seconds until the window resets (always 1..=60).
    pub fn check(&self, ip: IpAddr) -> std::result::Result<(), u64> {
        const WINDOW: Duration = Duration::from_secs(60);
        const PRUNE_AT: usize = 4096;
        let mut guard = self.window.lock().expect("rate-limit lock");
        if guard.len() > PRUNE_AT {
            guard.retain(|_, (started, _)| started.elapsed() < WINDOW);
        }
        let now = Instant::now();
        let entry = guard.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > self.limit_per_minute {
            let reset = WINDOW
                .checked_sub(now.duration_since(entry.0))
                .map(|d| d.as_secs().max(1))
                .unwrap_or(1);
            return Err(reset);
        }
        Ok(())
    }

    pub fn limit_per_minute(&self) -> u32 {
        self.limit_per_minute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn bootstrap_is_human_scoped_and_warns() {
        let store = TokenStore::bootstrap("bootstrap-secret-0123".to_string()).unwrap();
        assert!(store.is_bootstrap());
        assert!(store
            .bootstrap_warning()
            .unwrap()
            .contains("LABRYS_API_TOKENS"));
        let auth = store.authenticate("bootstrap-secret-0123").unwrap();
        assert_eq!(auth.scope, TokenScope::Human);
        assert!(store.authenticate("wrong-secret-value-00").is_err());
    }

    #[test]
    fn multi_token_expiry_and_revocation() {
        let store = TokenStore::from_lines(
            "ci:human:never:human-secret-0123456789\nrunner:agent:2999-01-01T00:00:00Z:agent-secret-0123456789\nold:human:2000-01-01T00:00:00Z:old-secret-01234567890",
        )
        .unwrap();
        assert!(!store.is_bootstrap());
        assert_eq!(store.len(), 3);
        assert_eq!(
            store.authenticate("human-secret-0123456789").unwrap().id,
            "ci"
        );
        assert_eq!(
            store.authenticate("agent-secret-0123456789").unwrap().scope,
            TokenScope::Agent
        );
        let denied = store.authenticate("old-secret-01234567890").unwrap_err();
        assert_eq!(denied.reason, AuthDenyReason::Expired);
        assert_eq!(denied.known_id.as_deref(), Some("old"));

        assert!(store.revoke("ci"));
        let denied = store.authenticate("human-secret-0123456789").unwrap_err();
        assert_eq!(denied.reason, AuthDenyReason::Revoked);
        assert_eq!(denied.known_id.as_deref(), Some("ci"));
        assert!(!store.revoke("ci"));
        assert!(!store.revoke("missing"));
    }

    #[test]
    fn reload_rotates_without_restart_and_rejects_bad_config() {
        let store = TokenStore::from_lines("a:human:never:aaaaaaaaaaaaaaaa").unwrap();
        assert!(store.authenticate("aaaaaaaaaaaaaaaa").is_ok());
        let count = store
            .reload(lookup(&[(
                "LABRYS_API_TOKENS",
                "b:human:never:bbbbbbbbbbbbbbbb",
            )]))
            .unwrap();
        assert_eq!(count, 1);
        assert!(store.authenticate("aaaaaaaaaaaaaaaa").is_err());
        assert!(store.authenticate("bbbbbbbbbbbbbbbb").is_ok());
        assert!(store
            .reload(lookup(&[("LABRYS_API_TOKENS", "broken-line")]))
            .is_err());
        assert!(store.authenticate("bbbbbbbbbbbbbbbb").is_ok());
    }

    #[test]
    fn token_lines_reject_malformed_entries() {
        assert!(TokenStore::from_lines("").is_err());
        assert!(TokenStore::from_lines("id:robot:never:aaaaaaaaaaaaaaaa").is_err());
        assert!(TokenStore::from_lines("id:human:never:short").is_err());
        assert!(TokenStore::from_lines("id:human:not-a-date:aaaaaaaaaaaaaaaa").is_err());
        assert!(TokenStore::from_lines(
            "a:human:never:aaaaaaaaaaaaaaaa\na:agent:never:bbbbbbbbbbbbbbbb"
        )
        .is_err());
    }

    #[test]
    fn unknown_token_fingerprint_reveals_nothing() {
        let fp = token_fingerprint("super-secret-token-value-00");
        assert!(fp.starts_with("sha256:"));
        assert!(!fp.contains("super-secret"));
        assert_eq!(token_fingerprint("super-secret-token-value-00"), fp);
    }

    #[test]
    fn rate_limiter_budgets_and_resets() {
        let limiter = RateLimiter::new(2);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(limiter.check(ip).is_ok());
        assert!(limiter.check(ip).is_ok());
        let retry = limiter.check(ip).unwrap_err();
        assert!((1..=60).contains(&retry));
        let other: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(limiter.check(other).is_ok());
    }

    #[test]
    fn bind_policy_matrix() {
        let loopback: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let public: std::net::SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert_eq!(
            bind_policy(&loopback, None, false).unwrap(),
            BindPolicy::PlainLoopback
        );
        assert!(bind_policy(&public, None, false).is_err());
        assert_eq!(
            bind_policy(&public, None, true).unwrap(),
            BindPolicy::PlainDisposable
        );
        let missing = TlsConfig {
            cert_file: PathBuf::from("/nonexistent/cert.pem"),
            key_file: PathBuf::from("/nonexistent/key.pem"),
        };
        assert!(bind_policy(&loopback, Some(&missing), false).is_err());
    }
}
