//! Control-plane API hardening: rotation without restart, revocation,
//! expiry, scope-bound actors, rate limits, TLS termination, and redacted
//! auth-failure audit — against an isolated `labrys_api_hardening_test`
//! database, skipping without `LABRYS_DATABASE_URL`.

use std::sync::{Arc, OnceLock};

use serde_json::Value;
use sqlx::{PgPool, Row};
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    run_migrations, serve_api, ApiConfig, ApiState, RateLimiter, TokenStore,
};

const HUMAN_TOKEN: &str = "hardening-human-token-0123456789";
const AGENT_TOKEN: &str = "hardening-agent-token-0123456789";
const EXPIRED_TOKEN: &str = "hardening-expired-token-0123456789";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

fn hardening_db_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    match base.rfind('/') {
        Some(idx) if idx > scheme_end => format!("{}/labrys_api_hardening_test", &base[..idx]),
        _ => format!("{base}/labrys_api_hardening_test"),
    }
}

async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let base = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let maintenance = PgPool::connect(&base).await.expect("connect postgres");
    let _ = sqlx::query("CREATE DATABASE labrys_api_hardening_test")
        .execute(&maintenance)
        .await;
    maintenance.close().await;
    let pool = PgPool::connect(&hardening_db_url(&base))
        .await
        .expect("connect hardening db");
    run_migrations(&pool).await.expect("run migrations");
    sqlx::query(
        "TRUNCATE applications, environments, desired_states, observed_states, resources, \
         deployments, capabilities, events, logs, audit_log, evidence, usage_records, jobs, \
         idempotency_records, provider_operations, registry_deliveries, domain_deliveries, \
         executions, previews RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    Some((guard, pool))
}

fn token_store() -> Arc<TokenStore> {
    TokenStore::from_lines(&format!(
        "test-human:human:never:{HUMAN_TOKEN}\ntest-agent:agent:never:{AGENT_TOKEN}\ntest-old:human:2000-01-01T00:00:00Z:{EXPIRED_TOKEN}"
    ))
    .expect("test tokens")
}

struct Server {
    base_url: String,
    http: reqwest::Client,
    tokens: Arc<TokenStore>,
}

impl Server {
    async fn start(pool: PgPool) -> Self {
        Self::start_with(pool, token_store(), DEFAULT_LIMIT).await
    }

    async fn start_with(pool: PgPool, tokens: Arc<TokenStore>, rate_limit: u32) -> Self {
        let mut state = ApiState::with_token_store(pool, tokens.clone(), Vec::new());
        state.rate_limiter = Arc::new(RateLimiter::new(rate_limit));
        let app = labrys_control_plane::api_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        Self {
            base_url: format!("http://{addr}"),
            http: reqwest::Client::new(),
            tokens,
        }
    }

    async fn raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        actor: &str,
    ) -> (u16, Value) {
        let mut request = match method {
            "GET" => self.http.get(format!("{}{}", self.base_url, path)),
            _ => self.http.post(format!("{}{}", self.base_url, path)),
        };
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request
            .header("x-labryst-actor", actor)
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("send");
        let status = response.status().as_u16();
        (status, response.json().await.expect("json"))
    }
}

const DEFAULT_LIMIT: u32 = 1000;

// ---------------------------------------------------------------------------
// Requirement: token auth supports rotation and revocation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rotated_token_takes_over_without_restart() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool).await;

    // Old and new tokens both live before rotation.
    let (status, _) = server
        .raw("GET", "/v1/plugins", Some(HUMAN_TOKEN), "human:op")
        .await;
    assert_eq!(status, 200);

    // WHEN configuration adds a replacement token and revokes the old one ...
    let rotated = server
        .tokens
        .reload(|key| match key {
            "LABRYS_API_TOKENS" => Some(format!(
                "test-human:human:never:{HUMAN_TOKEN}\ntest-agent:agent:never:{AGENT_TOKEN}\ntest-new:human:never:hardening-replacement-token-00"
            )),
            _ => None,
        })
        .expect("reload");
    assert_eq!(rotated, 3);
    assert!(server.tokens.revoke("test-human"));

    // THEN new requests succeed on the replacement ...
    let (status, _) = server
        .raw(
            "GET",
            "/v1/plugins",
            Some("hardening-replacement-token-00"),
            "human:op",
        )
        .await;
    assert_eq!(status, 200);
    // AND fail closed on the revoked token with recovery guidance.
    let (status, body) = server
        .raw("GET", "/v1/plugins", Some(HUMAN_TOKEN), "human:op")
        .await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], "api.unauthorized");
    assert!(!body["recovery"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn expired_token_is_denied_before_mutation_with_redacted_audit() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;

    // WHEN a request bears an expired token ...
    let (status, _body) = server
        .raw("POST", "/v1/import", Some(EXPIRED_TOKEN), "human:op")
        .await;
    // THEN the API denies it before any mutation ...
    assert_eq!(status, 401);
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("count")
        .try_get("count")
        .expect("count");
    assert_eq!(count, 0);
    // AND a redacted audit record names the token id without the value.
    let detail: String = sqlx::query(
        "SELECT detail FROM audit_log WHERE action = 'auth.rejected' ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("audit row")
    .try_get("detail")
    .expect("detail");
    assert!(detail.contains("expired"), "{detail}");
    assert!(detail.contains("test-old"), "{detail}");
    assert!(!detail.contains(EXPIRED_TOKEN), "{detail}");
}

#[tokio::test]
async fn unknown_expired_revoked_share_one_response_shape() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    server.tokens.revoke("test-human");

    // No identity oracle: the three denials share code, message, recovery.
    let shape = |body: &Value| {
        (
            body["code"].clone(),
            body["message"].clone(),
            body["recovery"].clone(),
        )
    };
    let (_, unknown) = server
        .raw(
            "GET",
            "/v1/plugins",
            Some("no-such-token-value-0000"),
            "human:op",
        )
        .await;
    let (_, expired) = server
        .raw("GET", "/v1/plugins", Some(EXPIRED_TOKEN), "human:op")
        .await;
    let (_, revoked) = server
        .raw("GET", "/v1/plugins", Some(HUMAN_TOKEN), "human:op")
        .await;
    assert_eq!(shape(&unknown), shape(&expired));
    assert_eq!(shape(&unknown), shape(&revoked));
}

// ---------------------------------------------------------------------------
// Requirement: actor identity binds to the credential
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_token_claiming_human_actor_is_denied_before_provider_work() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;

    // WHEN an agent-scoped token presents a human actor header ...
    let (status, body) = server
        .raw(
            "POST",
            "/v1/applications/00000000-0000-0000-0000-000000000000/deploy",
            Some(AGENT_TOKEN),
            "human:mallory",
        )
        .await;
    // THEN the request is denied before enqueueing provider work ...
    assert_eq!(status, 403, "{body}");
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("count")
        .try_get("count")
        .expect("count");
    assert_eq!(count, 0);
    // AND the denial carries the approval recovery path.
    let recovery = body["recovery"].as_str().unwrap_or("");
    assert!(recovery.contains("human"), "{body}");
    assert!(recovery.contains("approve"), "{body}");
    // AND the scope denial is audit-logged without secret material.
    let detail: String = sqlx::query(
        "SELECT detail FROM audit_log WHERE action = 'auth.rejected' ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("audit row")
    .try_get("detail")
    .expect("detail");
    assert!(detail.contains("test-agent"), "{detail}");
    assert!(!detail.contains(AGENT_TOKEN), "{detail}");
}

#[tokio::test]
async fn human_token_claiming_agent_actor_is_denied() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    let (status, body) = server
        .raw("GET", "/v1/plugins", Some(HUMAN_TOKEN), "agent:sess-1:bot")
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["code"], "api.forbidden");
}

#[tokio::test]
async fn matched_scope_pairs_keep_working() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    let (status, _) = server
        .raw("GET", "/v1/plugins", Some(HUMAN_TOKEN), "human:op")
        .await;
    assert_eq!(status, 200);
    let (status, _) = server
        .raw("GET", "/v1/plugins", Some(AGENT_TOKEN), "agent:sess-9:bot")
        .await;
    assert_eq!(status, 200);
}

// ---------------------------------------------------------------------------
// Rate limiting, TLS termination, bootstrap config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rate_limiter_returns_429_with_recovery() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start_with(pool, token_store(), 2).await;
    let (first, _) = server.raw("GET", "/v1/version", None, "human:op").await;
    assert_eq!(first, 200);
    let (second, _) = server.raw("GET", "/v1/version", None, "human:op").await;
    assert_eq!(second, 200);
    let (status, body) = server.raw("GET", "/v1/version", None, "human:op").await;
    assert_eq!(status, 429, "{body}");
    assert_eq!(body["code"], "api.rate_limited");
    assert!(!body["recovery"].as_str().unwrap_or("").is_empty());
}

#[test]
fn api_config_rejects_risky_binds_and_reports_bootstrap() {
    // Loopback bootstrap is the disposable default and warns.
    let config = ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKEN" => Some("bootstrap-token-0123456789".to_string()),
        _ => None,
    })
    .expect("bootstrap config");
    assert!(config.auth_summary().contains("WARNING"));

    // Scoped tokens report ids, never values.
    let config = ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKENS" => Some(format!("a:human:never:{HUMAN_TOKEN}")),
        _ => None,
    })
    .expect("scoped config");
    let summary = config.auth_summary();
    assert!(summary.contains('a'));
    assert!(!summary.contains(HUMAN_TOKEN));

    // Non-loopback plain HTTP is refused without the explicit flag.
    let err = ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKEN" => Some("bootstrap-token-0123456789".to_string()),
        "LABRYS_API_ADDR" => Some("0.0.0.0:8080".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(err.to_string().contains("LABRYS_TLS_CERT_FILE"));

    // ... and allowed with it.
    let config = ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKEN" => Some("bootstrap-token-0123456789".to_string()),
        "LABRYS_API_ADDR" => Some("0.0.0.0:8080".to_string()),
        "LABRYS_ALLOW_PLAIN_HTTP" => Some("1".to_string()),
        _ => None,
    })
    .expect("disposable config");
    assert!(config.allow_plain_http);

    // Half-configured TLS is refused.
    assert!(ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKEN" => Some("bootstrap-token-0123456789".to_string()),
        "LABRYS_TLS_CERT_FILE" => Some("/tmp/cert.pem".to_string()),
        _ => None,
    })
    .is_err());
}

#[tokio::test]
async fn tls_termination_serves_localhost_with_rcgen_cert() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    // Locally issued cert for 127.0.0.1, mirroring staged environments.
    let certified =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).expect("cert");
    let dir = std::env::temp_dir().join("labrys-api-hardening-tls");
    std::fs::create_dir_all(&dir).expect("tls dir");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, certified.cert.pem()).expect("cert");
    std::fs::write(&key_path, certified.key_pair.serialize_pem()).expect("key");

    let port = free_port();
    let config = ApiConfig::from_lookup(|key| match key {
        "LABRYS_API_TOKEN" => Some("bootstrap-token-0123456789".to_string()),
        "LABRYS_API_ADDR" => Some(format!("127.0.0.1:{port}")),
        "LABRYS_TLS_CERT_FILE" => Some(cert_path.display().to_string()),
        "LABRYS_TLS_KEY_FILE" => Some(key_path.display().to_string()),
        _ => None,
    })
    .expect("tls config");
    let state = ApiState::with_token_store(pool, config.tokens.clone(), Vec::new());
    let mut server = tokio::spawn(async move { serve_api(&config, state).await });
    tokio::select! {
        result = &mut server => panic!("TLS server exited early: {result:?}"),
        _ = wait_for_tcp(port) => {}
    }

    // The dashboard/CLI path: trust the local CA explicitly.
    let ca = reqwest::Certificate::from_pem(certified.cert.pem().as_bytes()).expect("ca");
    let http = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .expect("client");
    let response = http
        .get(format!("https://127.0.0.1:{port}/v1/version"))
        .send()
        .await
        .expect("tls request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json");
    assert_eq!(body["versions"]["api"], 1);

    // Plain HTTP against the TLS port is not valid HTTP.
    let plain = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/v1/version"))
        .send()
        .await;
    assert!(plain.is_err() || plain.map(|r| r.status().as_u16()).unwrap_or(0) != 200);

    server.abort();
    std::fs::remove_dir_all(&dir).ok();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

async fn wait_for_tcp(port: u16) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("TLS server did not start on {port}");
}
