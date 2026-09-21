//! API + CLI integration tests for the executable control plane boundary.
//!
//! These exercise real HTTP routes, durable idempotency, actor authorization,
//! and the `labrys` binary as a subprocess against an isolated
//! `labrys_api_cli_test` database, and skip without `LABRYS_DATABASE_URL`.
//! They are infrastructure evidence, not production-deployment evidence: no
//! test performs real provider or runtime work — mutations return accepted
//! job references until platform observations establish readiness.

use std::sync::OnceLock;

use chrono::Utc;
use serde_json::Value;
use sqlx::{PgPool, Row};
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    run_migrations, ApiState, PgApplicationStore, PgDeploymentStore, PgEventStore, PgLogStore,
};
use labrys_core::{ApplicationId, Plugin, PLUGIN_PROTOCOL_VERSION};

const TOKEN: &str = "test-api-token-0123456789";
const SECRET: &str = "api-cli-test-secret-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

fn api_db_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    match base.rfind('/') {
        Some(idx) if idx > scheme_end => format!("{}/labrys_api_cli_test", &base[..idx]),
        _ => format!("{base}/labrys_api_cli_test"),
    }
}

async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let base = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let maintenance = PgPool::connect(&base).await.expect("connect postgres");
    let _ = sqlx::query("CREATE DATABASE labrys_api_cli_test")
        .execute(&maintenance)
        .await;
    maintenance.close().await;
    let pool = PgPool::connect(&api_db_url(&base))
        .await
        .expect("connect api db");
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

struct Server {
    base_url: String,
    http: reqwest::Client,
}

impl Server {
    async fn start(pool: PgPool) -> Self {
        let state = ApiState::new(pool, TOKEN.to_string());
        let app = labrys_control_plane::api_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self {
            base_url: format!("http://{addr}"),
            http: reqwest::Client::new(),
        }
    }

    async fn get(&self, path: &str, actor: &str) -> (u16, Value) {
        let response = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("x-labryst-actor", actor)
            .send()
            .await
            .expect("send");
        let status = response.status().as_u16();
        (status, response.json().await.expect("json"))
    }

    async fn post(&self, path: &str, actor: &str, key: Option<&str>, body: &Value) -> (u16, Value) {
        let mut request = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("x-labryst-actor", actor);
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request.json(body).send().await.expect("send");
        let status = response.status().as_u16();
        (status, response.json().await.expect("json"))
    }
}

async fn import_app(server: &Server, name: &str, key: &str) -> String {
    let (status, body) = server
        .post(
            "/v1/import",
            "human:tester",
            Some(key),
            &serde_json::json!({ "name": name, "path": "." }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    body["data"]["application_id"]
        .as_str()
        .expect("app id")
        .to_string()
}

fn versions_of(body: &Value) -> &Value {
    body.get("versions").expect("versions")
}

// ---------------------------------------------------------------------------
// Version negotiation and authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn version_is_public_and_negotiation_enforces_protocol() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool).await;

    // GET /v1/version needs no token: negotiation happens before auth.
    let response = server
        .http
        .get(format!("{}/v1/version", server.base_url))
        .send()
        .await
        .expect("send");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json");
    assert_eq!(body["versions"]["api"], 1);
    assert_eq!(body["versions"]["agent"], 1);
    assert_eq!(body["versions"]["plugin"], PLUGIN_PROTOCOL_VERSION);

    // Agent negotiation accepts the current protocol ...
    let (status, body) = server
        .post(
            "/v1/agents/negotiate",
            "human:tester",
            None,
            &serde_json::json!({"protocol_version": 1, "backend": "native"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["negotiated_version"], 1);

    // ... and refuses anything else with a recovery path.
    let (status, body) = server
        .post(
            "/v1/agents/negotiate",
            "human:tester",
            None,
            &serde_json::json!({"protocol_version": 99}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["ok"], false);
    assert!(body["recovery"].as_str().unwrap_or("").contains("upgrade"));

    // Authenticated routes reject missing tokens with a recovery path.
    let response = server
        .http
        .get(format!(
            "{}/v1/applications/{}/inspect",
            server.base_url,
            ApplicationId::new()
        ))
        .header("x-labryst-actor", "human:tester")
        .send()
        .await
        .expect("send");
    assert_eq!(response.status().as_u16(), 401);
    let body: Value = response.json().await.expect("json");
    assert!(body["recovery"]
        .as_str()
        .unwrap_or("")
        .contains("LABRYS_API_TOKEN"));

    // Malformed actors are rejected before any state is touched.
    let (status, body) = server
        .get(
            &format!("/v1/applications/{}/inspect", ApplicationId::new()),
            "nonsense",
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "api.actor_required");
}

// ---------------------------------------------------------------------------
// Import → inspect → doctor, with durable idempotent replay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_inspect_doctor_flow_with_replay() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;

    let app = import_app(&server, "shop", "key-import-1").await;

    let (status, body) = server
        .get(&format!("/v1/applications/{app}/inspect"), "human:tester")
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["data"]["environments"].as_array().is_some());
    assert!(versions_of(&body).is_object());

    // Doctor reports missing health evidence as a warning, not an error.
    let (status, body) = server
        .get(&format!("/v1/applications/{app}/doctor"), "human:tester")
        .await;
    assert_eq!(status, 200, "{body}");
    let checks = body["data"]["checks"].as_array().expect("checks");
    assert!(checks
        .iter()
        .any(|c| c["code"] == "doctor.no_health_evidence"));

    // Retrying import with the same key replays one durable operation.
    let (status, replay) = server
        .post(
            "/v1/import",
            "human:tester",
            Some("key-import-1"),
            &serde_json::json!({"name": "shop", "path": "."}),
        )
        .await;
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"]["application_id"].as_str().unwrap(), app);

    // Exactly one import job exists: the side effect executed once.
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("count")
        .try_get("count")
        .expect("count");
    assert_eq!(count, 1);

    // Mutations without a key are rejected before enqueueing.
    let (status, body) = server
        .post(
            "/v1/import",
            "human:tester",
            None,
            &serde_json::json!({"name": "x"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "api.idempotency_required");
}

// ---------------------------------------------------------------------------
// Agent deploy denial, deploy replay, in-progress health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_production_deploy_is_rejected_before_provider_work() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app = import_app(&server, "shop", "key-import-agent").await;

    // WHEN an agent calls the production deploy endpoint without approval ...
    let (status, body) = server
        .post(
            &format!("/v1/applications/{app}/deploy"),
            "agent:sess-1:bot",
            Some("key-deploy-agent"),
            &serde_json::json!({"environment": "production"}),
        )
        .await;

    // THEN the API rejects the mutation before enqueueing provider work ...
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["ok"], false);
    // AND the response contains a recovery path for human approval.
    let recovery = body["recovery"].as_str().unwrap_or("");
    assert!(recovery.contains("human"), "{body}");
    assert!(body["correlation"]["trace_id"].is_string());

    // No job was enqueued: zero provider work.
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs WHERE target LIKE '%deploy%'")
        .fetch_one(&pool)
        .await
        .expect("count")
        .try_get("count")
        .expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn deploy_retry_replays_one_durable_operation() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app = import_app(&server, "shop", "key-import-deploy").await;

    let deploy = serde_json::json!({"environment": "production"});
    let (status, first) = server
        .post(
            &format!("/v1/applications/{app}/deploy"),
            "human:ada",
            Some("key-deploy-1"),
            &deploy,
        )
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["ok"], true);
    let job_id = first["job"]["id"].as_str().expect("job").to_string();

    // WHEN the same idempotency key is submitted twice ...
    let (status, second) = server
        .post(
            &format!("/v1/applications/{app}/deploy"),
            "human:ada",
            Some("key-deploy-1"),
            &deploy,
        )
        .await;
    assert_eq!(status, 200, "{second}");

    // THEN both responses identify one durable operation ...
    assert_eq!(second["replayed"], true);
    assert_eq!(second["job"]["id"].as_str().unwrap(), job_id);
    // AND the side effect executes once.
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs WHERE target LIKE '%deploy%'")
        .fetch_one(&pool)
        .await
        .expect("count")
        .try_get("count")
        .expect("count");
    assert_eq!(count, 1);

    // Job status is queryable with structured state.
    let (status, job) = server.get(&format!("/v1/jobs/{job_id}"), "human:ada").await;
    assert_eq!(status, 200, "{job}");
    assert_eq!(job["job"]["action"], "rollout");
}

#[tokio::test]
async fn accepted_deploy_reports_in_progress_never_healthy() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    let app = import_app(&server, "shop", "key-import-health").await;
    let deploy = serde_json::json!({"environment": "production"});
    let (status, _) = server
        .post(
            &format!("/v1/applications/{app}/deploy"),
            "human:ada",
            Some("key-deploy-h"),
            &deploy,
        )
        .await;
    assert_eq!(status, 200);

    // WHEN a deployment request is accepted but health is not observed ...
    let (status, body) = server
        .get(
            &format!("/v1/applications/{app}/health?environment=production"),
            "human:ada",
        )
        .await;
    assert_eq!(status, 200, "{body}");

    // THEN the API reports an in-progress state and job reference ...
    assert_eq!(body["data"]["state"], "in_progress", "{body}");
    assert!(body["data"]["environments"][0]["job"].is_object(), "{body}");
    // AND it does not report production healthy.
    assert_ne!(body["data"]["state"], "healthy");
}

// ---------------------------------------------------------------------------
// Rollback: agent denial, approval requirement, happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rollback_boundaries_hold_over_http() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app = import_app(&server, "shop", "key-import-rb").await;

    // WHEN an agent invokes rollback without named human approval ...
    let (status, body) = server
        .post(
            &format!("/v1/applications/{app}/rollback"),
            "agent:sess-1:bot",
            Some("key-rb-agent"),
            &serde_json::json!({"target": "dep_x", "approval": {"approver": "bot"}}),
        )
        .await;
    // THEN the command fails without enqueuing rollback ...
    assert_eq!(status, 403, "{body}");
    // AND the output states the approval requirement.
    assert!(
        body["message"].as_str().unwrap_or("").contains("approval"),
        "{body}"
    );
    assert!(
        body["recovery"]
            .as_str()
            .unwrap_or("")
            .contains("approve-by"),
        "{body}"
    );

    // A human without approval is rejected too.
    let (status, body) = server
        .post(
            &format!("/v1/applications/{app}/rollback"),
            "human:ada",
            Some("key-rb-noap"),
            &serde_json::json!({"target": "dep_x"}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "api.approval_required");
}

#[tokio::test]
async fn rollback_happy_path_warns_about_database_data() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app_id = import_app(&server, "shop", "key-import-rb2").await;

    // A healthy, built deployment as the rollback target.
    let app_uuid: ApplicationId = app_id.parse().expect("app id");
    let stored = PgApplicationStore::new(pool.clone())
        .get(&app_uuid)
        .await
        .expect("app");
    let env = stored
        .environments
        .iter()
        .find(|e| e.name == "production")
        .expect("production env")
        .clone();
    let at = Utc::now();
    let mut deployment = labrys_core::Deployment::new(
        app_uuid,
        env.id,
        labrys_core::Revision::new("abc123", "cfg", Some("cap".to_string())),
        labrys_core::RuntimeConfig {
            kind: labrys_core::RuntimeKind::Node,
            tier: labrys_core::SupportTier::Tier1,
            profile: labrys_core::RuntimeProfile::Production,
            port: 8080,
            healthcheck_path: Some("/healthz".to_string()),
            dockerfile: None,
            run_command: vec!["serve".to_string()],
            oci_image: Some("registry.local/app".to_string()),
            limits: labrys_core::SandboxLimits::docker_default(),
        },
        vec![],
        None,
        at,
    )
    .expect("deployment");
    deployment.start_build(at).expect("build");
    deployment
        .record_build(&labrys_core::BuildResult::succeeded("ok"), &[], at)
        .expect("record");
    deployment
        .attach_endpoint(labrys_core::Endpoint::loopback(8080, true).expect("ep"))
        .expect("ep");
    deployment
        .record_health(labrys_core::HealthStatus::Healthy, &[], at)
        .expect("health");
    PgDeploymentStore::new(pool.clone())
        .save(&deployment)
        .await
        .expect("save");

    let (status, body) = server
        .post(
            &format!("/v1/applications/{app_id}/rollback"),
            "human:ada",
            Some("key-rb-ok"),
            &serde_json::json!({"target": deployment.id.to_string(), "approval": {"approver": "ada"}}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["data"]["database_data_restored"], false);
    assert_eq!(body["data"]["approved_by"], "ada");
    assert!(body["job"]["id"].is_string());
}

// ---------------------------------------------------------------------------
// Logs redaction + pagination, deployments pagination, unknown app
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logs_are_redacted_and_paginated() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app = import_app(&server, "shop", "key-import-logs").await;

    PgLogStore::new(pool.clone())
        .append(
            labrys_core::LogSource::Deployment,
            labrys_core::LogLevel::Error,
            &labrys_core::Correlation::new("trace-logs"),
            &format!("deploy failed with password {SECRET}"),
            &[SECRET],
            Utc::now(),
        )
        .await
        .expect("append");

    let (status, body) = server
        .get(&format!("/v1/applications/{app}/logs?limit=1"), "human:ada")
        .await;
    assert_eq!(status, 200, "{body}");
    let items = body["data"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert!(body["data"]["total"].as_i64().unwrap() >= 1);
    let rendered = serde_json::to_string(&body).expect("render");
    assert!(!rendered.contains(SECRET), "{body}");
    assert!(rendered.contains("[redacted]"), "{body}");
}

#[tokio::test]
async fn unknown_application_reports_recovery() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    let missing = ApplicationId::new();
    let (status, body) = server
        .get(&format!("/v1/applications/{missing}/inspect"), "human:ada")
        .await;
    assert_eq!(status, 404, "{body}");
    assert!(
        body["recovery"].as_str().unwrap_or("").contains("import"),
        "{body}"
    );

    let (status, body) = server
        .get(&format!("/v1/applications/{missing}/health"), "human:ada")
        .await;
    assert_eq!(status, 404, "{body}");
}

// ---------------------------------------------------------------------------
// Plugin handshake and lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_handshake_accepts_contract_and_refuses_mismatch() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;

    let manifest = labrys_core::ExampleCapabilityProviderPlugin.manifest();
    let (status, body) = server
        .post(
            "/v1/plugins/handshake",
            "human:ada",
            None,
            &serde_json::json!({"manifest": manifest}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["negotiated_version"], PLUGIN_PROTOCOL_VERSION);

    let mut bad = labrys_core::ExampleCapabilityProviderPlugin.manifest();
    bad.id = "example-capability-bad".to_string();
    bad.protocol_version = 99;
    let (status, body) = server
        .post(
            "/v1/plugins/handshake",
            "human:ada",
            None,
            &serde_json::json!({"manifest": bad}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["recovery"].as_str().unwrap_or("").contains("upgrade"),
        "{body}"
    );

    let (status, body) = server.get("/v1/plugins", "human:ada").await;
    assert_eq!(status, 200, "{body}");
    let items = body["data"]["items"].as_array().expect("items");
    assert!(items.iter().any(|p| p["state"] == "active"), "{body}");
}

// ---------------------------------------------------------------------------
// CLI black-box: real binary against a real server
// ---------------------------------------------------------------------------

fn labrys_bin() -> String {
    env!("CARGO_BIN_EXE_labrys").to_string()
}

#[tokio::test]
async fn cli_doctor_and_agent_rollback_black_box() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let server = Server::start(_pool).await;
    let app = import_app(&server, "shop", "key-import-cli").await;
    let bin = labrys_bin();

    // Operator diagnoses the imported project: structured findings.
    let output = tokio::process::Command::new(&bin)
        .args([
            "--api-url",
            &server.base_url,
            "--token",
            TOKEN,
            "--actor",
            "human:tester",
            "doctor",
            "--application",
            &app,
        ])
        .output()
        .await
        .expect("run labrys doctor");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["ok"], true);
    assert!(envelope["data"]["checks"].is_array());

    // Agent rollback through the CLI fails with the approval requirement.
    let output = tokio::process::Command::new(&bin)
        .args([
            "--api-url",
            &server.base_url,
            "--token",
            TOKEN,
            "--actor",
            "agent:sess-1:bot",
            "rollback",
            "--application",
            &app,
            "--target",
            "dep_x",
            "--approve-by",
            "bot",
        ])
        .output()
        .await
        .expect("run labrys rollback");
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("approval"), "{combined}");

    // CLI without a token fails with usage guidance, touching no server state.
    let output = tokio::process::Command::new(&bin)
        .args([
            "--api-url",
            &server.base_url,
            "doctor",
            "--application",
            &app,
        ])
        .env("LABRYS_API_TOKEN", "")
        .output()
        .await
        .expect("run without token");
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Attribution: events carry the calling actor and trace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutations_emit_attributed_correlated_events() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let server = Server::start(pool.clone()).await;
    let app = import_app(&server, "shop", "key-import-attr").await;

    let response = server
        .http
        .post(format!("{}/v1/applications/{app}/build", server.base_url))
        .header("Authorization", format!("Bearer {TOKEN}"))
        .header("x-labryst-actor", "human:ada")
        .header("x-trace-id", "trace-attr-1")
        .header("Idempotency-Key", "key-build-attr")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("send");
    assert_eq!(response.status().as_u16(), 200);

    let events = PgEventStore::new(pool.clone())
        .for_trace("trace-attr-1")
        .await
        .expect("events");
    assert!(!events.is_empty());
    assert!(events
        .iter()
        .all(|e| e.correlation.trace_id == "trace-attr-1"));
    let rendered = serde_json::to_string(&events).expect("render");
    assert!(
        rendered.contains("ada") || rendered.contains("Human"),
        "{rendered}"
    );
}
