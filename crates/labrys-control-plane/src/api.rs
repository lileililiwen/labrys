//! Authenticated Axum control-plane API over the durable stores.
//!
//! This module turns the pure `labrys-core` contracts into an executable
//! process boundary: handlers validate actor scope, create idempotent
//! commands, enqueue controller jobs, and return structured status rather
//! than performing provider work inline. Mutations return accepted/job
//! references until platform observations establish readiness; reads serve
//! redacted, attributable evidence from the repositories.
//!
//! Boundaries:
//! - Every route except `GET /v1/version` requires a bearer token
//!   (`LABRYS_API_TOKEN`); the actor travels in `x-labryst-actor`
//!   (`human:<name>`, `agent:<session>:<name>`, `platform:<name>`).
//! - Agent actors cannot invoke human-only lifecycle mutations (import,
//!   build, deploy, rollback); rollback additionally requires a granted,
//!   named human approval. Denials happen before any job is enqueued and
//!   carry a recovery path.
//! - Mutating commands require an `Idempotency-Key`; the response is stored
//!   durably under that key, so a retry replays one recorded operation and
//!   the side effect executes once.
//! - Responses carry correlation, resource identity, structured recovery,
//!   and protocol versions, and never secret values: free text is redacted
//!   before persistence and only references or markers leave the API.
//! - A passing handler test does not prove provider/runtime delivery; this
//!   surface reports platform observations, it does not manufacture them.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use labrys_core::{
    Application, ApplicationId, Correlation, DeploymentId, DeploymentPhase, EnqueueOutcome,
    EventAction, EventActor, EventDraft, EventResource, EventResult, ExplicitApproval, Job,
    JobAction, JobId, LogSource, Origin, PluginManifest, RetryPolicy, AGENT_PROTOCOL_VERSION,
    PLUGIN_PROTOCOL_VERSION,
};

use crate::error::{ControlPlaneError, Result};
use crate::jobs::PgJobQueue;
use crate::observability::{PgAuditLog, PgEventStore, PgEvidenceStore, PgLogStore};
use crate::repos::{PgApplicationStore, PgCapabilityStore, PgDeploymentStore, PgResourceStore};
use crate::{redact, Pool};

/// Version of the HTTP API surface this crate serves.
pub const API_VERSION: u32 = 1;

/// Process configuration for the API server. The bearer token comes from
/// process configuration, never from a manifest or agent prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    pub token: String,
    pub bind_addr: SocketAddr,
}

impl ApiConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let token = lookup("LABRYS_API_TOKEN").unwrap_or_default();
        let bind_addr: SocketAddr = match lookup("LABRYS_API_ADDR") {
            None => "127.0.0.1:8080".parse().expect("default addr parses"),
            Some(raw) => raw.parse().map_err(|_| {
                ControlPlaneError::Config(format!("LABRYS_API_ADDR is not a socket address: {raw}"))
            })?,
        };
        let config = Self { token, bind_addr };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.token.trim().is_empty() {
            return Err(ControlPlaneError::Config(
                "API token is required (LABRYS_API_TOKEN)".to_string(),
            ));
        }
        if self.token.len() < 16 {
            return Err(ControlPlaneError::Config(
                "API token must be at least 16 characters".to_string(),
            ));
        }
        Ok(())
    }
}

/// Authenticated request context: who is calling, which trace ties their
/// events together, and which idempotency key scopes a retried mutation.
#[derive(Debug, Clone)]
pub struct ActorContext {
    pub actor: labrys_core::CliActor,
    pub trace_id: String,
    pub idempotency_key: Option<String>,
}

impl ActorContext {
    pub fn event_actor(&self) -> EventActor {
        match &self.actor {
            labrys_core::CliActor::Human { name } => EventActor::Human { name: name.clone() },
            labrys_core::CliActor::Platform { name } => EventActor::Platform { name: name.clone() },
            labrys_core::CliActor::Agent { session_id, name } => session_id
                .parse()
                .map(|session_id| EventActor::Agent {
                    session_id,
                    name: name.clone(),
                })
                .unwrap_or_else(|_| EventActor::Platform {
                    name: format!("agent:{name}"),
                }),
        }
    }

    pub fn correlation(&self) -> Correlation {
        Correlation::new(self.trace_id.clone())
    }
}

/// Shared handler state. Everything here is Clone so Axum can share it.
#[derive(Clone)]
pub struct ApiState {
    pub pool: Pool,
    pub jobs: PgJobQueue,
    pub apps: PgApplicationStore,
    pub resources: PgResourceStore,
    pub deployments: PgDeploymentStore,
    pub capabilities: PgCapabilityStore,
    pub events: PgEventStore,
    pub logs: PgLogStore,
    pub audit: PgAuditLog,
    pub evidence: PgEvidenceStore,
    pub plugins: Arc<tokio::sync::Mutex<labrys_core::PluginRegistry>>,
    pub token: String,
    pub secrets: Vec<String>,
}

impl ApiState {
    pub fn new(pool: Pool, token: String) -> Self {
        Self::with_secrets(pool, token, Vec::new())
    }

    pub fn with_secrets(pool: Pool, token: String, secrets: Vec<String>) -> Self {
        Self {
            jobs: PgJobQueue::new(pool.clone()),
            apps: PgApplicationStore::new(pool.clone()),
            resources: PgResourceStore::new(pool.clone()),
            deployments: PgDeploymentStore::new(pool.clone()),
            capabilities: PgCapabilityStore::new(pool.clone()),
            events: PgEventStore::new(pool.clone()),
            logs: PgLogStore::new(pool.clone()),
            audit: PgAuditLog::new(pool.clone()),
            evidence: PgEvidenceStore::new(pool.clone(), labrys_core::RetentionPolicy::default()),
            plugins: Arc::new(tokio::sync::Mutex::new(labrys_core::PluginRegistry::new())),
            pool,
            token,
            secrets,
        }
    }

    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }
}

/// Structured API failure: always JSON, always with a recovery path.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub recovery: String,
    pub trace_id: String,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        recovery: impl Into<String>,
        trace_id: &str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            recovery: recovery.into(),
            trace_id: trace_id.to_string(),
        }
    }

    fn unauthorized(trace_id: &str) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "api.unauthorized",
            "missing or invalid API token",
            "set LABRYS_API_TOKEN (server) and pass --token <token> or Authorization: Bearer <token>",
            trace_id,
        )
    }

    fn forbidden(trace_id: &str, message: String, recovery: &str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "api.forbidden",
            message,
            recovery,
            trace_id,
        )
    }

    fn not_found(trace_id: &str, message: String, recovery: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "api.not_found",
            message,
            recovery,
            trace_id,
        )
    }

    fn unprocessable(trace_id: &str, message: String, recovery: &str) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "api.unprocessable",
            message,
            recovery,
            trace_id,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "ok": false,
            "replayed": false,
            "correlation": { "trace_id": self.trace_id },
            "code": self.code,
            "message": self.message,
            "recovery": self.recovery,
            "versions": versions_value(),
        });
        (self.status, Json(body)).into_response()
    }
}

fn versions_value() -> serde_json::Value {
    serde_json::json!({
        "api": API_VERSION,
        "agent": AGENT_PROTOCOL_VERSION,
        "plugin": PLUGIN_PROTOCOL_VERSION,
    })
}

fn trace_of(headers: &HeaderMap) -> String {
    headers
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("trace-{}", uuid::Uuid::new_v4().simple()))
}

fn idempotency_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Extracts and validates the bearer token plus actor context. Called at the
/// top of every authenticated handler.
fn actor_context(
    state: &ApiState,
    headers: &HeaderMap,
    trace_id: &str,
) -> std::result::Result<ActorContext, ApiError> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if bearer != state.token {
        return Err(ApiError::unauthorized(trace_id));
    }
    let raw = headers
        .get("x-labryst-actor")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let actor = parse_actor(raw).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "api.actor_required",
            "missing or malformed actor identity",
            "pass x-labryst-actor: human:<name>, agent:<session>:<name>, or platform:<name>",
            trace_id,
        )
    })?;
    Ok(ActorContext {
        actor,
        trace_id: trace_id.to_string(),
        idempotency_key: idempotency_of(headers),
    })
}

fn parse_actor(raw: &str) -> Option<labrys_core::CliActor> {
    let mut parts = raw.splitn(3, ':');
    match (parts.next()?, parts.next(), parts.next()) {
        (kind, Some(name), None) if kind == "human" && !name.trim().is_empty() => {
            Some(labrys_core::CliActor::human(name))
        }
        (kind, Some(name), None) if kind == "platform" && !name.trim().is_empty() => {
            Some(labrys_core::CliActor::Platform {
                name: name.to_string(),
            })
        }
        (kind, Some(session), Some(name))
            if kind == "agent" && !session.trim().is_empty() && !name.trim().is_empty() =>
        {
            Some(labrys_core::CliActor::agent(session, name))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Envelopes, jobs, idempotency
// ---------------------------------------------------------------------------

fn job_value(job: &Job) -> serde_json::Value {
    serde_json::json!({
        "id": job.id.to_string(),
        "target": job.target,
        "action": job.action.as_str(),
        "status": format!("{:?}", job.status).to_lowercase(),
        "attempts": job.attempts,
        "idempotency_key": job.idempotency_key.to_string(),
    })
}

fn envelope(
    command: &str,
    ctx: &ActorContext,
    replayed: bool,
    resource: Option<serde_json::Value>,
    data: Option<serde_json::Value>,
    job: Option<&Job>,
    recovery: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "replayed": replayed,
        "command": command,
        "correlation": { "trace_id": ctx.trace_id },
        "resource": resource,
        "data": data,
        "job": job.map(job_value),
        "recovery": recovery,
        "versions": versions_value(),
    })
}

async fn check_replay(state: &ApiState, key: Option<&str>) -> Result<Option<serde_json::Value>> {
    let Some(key) = key else { return Ok(None) };
    let row: Option<serde_json::Value> =
        sqlx::query("SELECT response FROM idempotency_records WHERE key = $1")
            .bind(key)
            .fetch_optional(&state.pool)
            .await?
            .map(|row| {
                use sqlx::Row;
                row.try_get("response").expect("response is jsonb")
            });
    Ok(row.map(|mut response| {
        response["replayed"] = serde_json::Value::Bool(true);
        response
    }))
}

async fn store_replay(
    state: &ApiState,
    key: &str,
    command: &str,
    response: &serde_json::Value,
) -> Result<()> {
    let detail = redact::redact_guard(
        "idempotency_records.response",
        &response.to_string(),
        &state.secret_refs(),
    )?;
    let response: serde_json::Value =
        serde_json::from_str(&detail).unwrap_or(serde_json::Value::Null);
    sqlx::query(
        "INSERT INTO idempotency_records (key, command, response, created_at) VALUES ($1,$2,$3,$4) ON CONFLICT (key) DO NOTHING",
    )
    .bind(key)
    .bind(command)
    .bind(&response)
    .bind(chrono::Utc::now())
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn enqueue(
    state: &ApiState,
    target: &str,
    action: JobAction,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(Job, bool)> {
    match state
        .jobs
        .enqueue(target, action, 1, RetryPolicy::default(), at)
        .await?
    {
        EnqueueOutcome::Enqueued(id) => Ok((state.jobs.get(&id).await?, false)),
        EnqueueOutcome::Duplicate(id) => Ok((state.jobs.get(&id).await?, true)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_event(
    state: &ApiState,
    ctx: &ActorContext,
    action: EventAction,
    result: EventResult,
    resource_kind: &str,
    resource_id: &str,
    detail: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let mut draft = EventDraft::new(at, ctx.event_actor(), ctx.correlation(), action, result);
    draft.resource = Some(EventResource::new(resource_kind, resource_id));
    draft.after = Some(detail.to_string());
    state.events.append(&draft, &state.secret_refs()).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Actor policy (mirrors the stable CLI contract in labrys-core)
// ---------------------------------------------------------------------------

fn forbid_agent(ctx: &ActorContext, command: &str) -> Option<ApiError> {
    if matches!(ctx.actor, labrys_core::CliActor::Agent { .. }) {
        return Some(ApiError::forbidden(
            &ctx.trace_id,
            format!("an agent session may not run '{command}'"),
            "a human operator must run this command; agents may request approval first",
        ));
    }
    None
}

fn require_human(ctx: &ActorContext, command: &str) -> Option<ApiError> {
    if !ctx.actor.is_human() {
        return Some(ApiError::forbidden(
            &ctx.trace_id,
            format!("'{command}' is a human-only operation"),
            "a named human must issue this command with --approve-by <name>",
        ));
    }
    None
}

fn require_idempotency(ctx: &ActorContext, command: &str) -> std::result::Result<String, ApiError> {
    ctx.idempotency_key.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "api.idempotency_required",
            format!("'{command}' changes platform state and needs an idempotency key"),
            "re-send with an Idempotency-Key header so a retry is safe",
            &ctx.trace_id,
        )
    })
}

async fn load_app(
    state: &ApiState,
    ctx: &ActorContext,
    raw: &str,
) -> std::result::Result<Application, ApiError> {
    let id: ApplicationId = raw.parse().map_err(|_| {
        ApiError::not_found(
            &ctx.trace_id,
            format!("application '{raw}' is not registered"),
            "run `labrys import` to register this project, then `labrys inspect`",
        )
    })?;
    state.apps.get(&id).await.map_err(|_| {
        ApiError::not_found(
            &ctx.trace_id,
            format!("application '{raw}' is not registered"),
            "run `labrys import` to register this project, then `labrys inspect`",
        )
    })
}

fn environment_id_of(app: &Application, name: &str) -> Option<labrys_core::EnvironmentId> {
    app.environments
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.id)
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ImportBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovalBody {
    approver: String,
}

#[derive(Debug, Deserialize)]
struct MutationBody {
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    approval: Option<ApprovalBody>,
    #[serde(default)]
    capability: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    revision_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    environment: Option<String>,
}

fn clamp_page(query: &PageQuery) -> (usize, usize) {
    let limit = query.limit.unwrap_or(50).clamp(1, 500) as usize;
    let offset = query.offset.unwrap_or(0).max(0) as usize;
    (limit, offset)
}

#[derive(Debug, Deserialize)]
struct NegotiateBody {
    protocol_version: u32,
    #[serde(default)]
    backend: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HandshakeBody {
    manifest: PluginManifest,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "versions": versions_value(),
    }))
}

async fn import(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<ImportBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    if let Some(err) = forbid_agent(&ctx, "import") {
        return Err(err);
    }
    let key = require_idempotency(&ctx, "import")?;
    if let Some(replayed) = check_replay(&state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let now = chrono::Utc::now();
    let name = body.name.unwrap_or_else(|| "imported-app".to_string());
    let path = body.path.unwrap_or_else(|| ".".to_string());
    let mut app = Application::new(
        name.clone(),
        Origin::ImportedLocal { path: path.clone() },
        ctx.actor.kind_label(),
    );
    state.apps.save(&app).await.map_err(internal(&ctx))?;
    let (job, _) = enqueue(&state, &app.id.to_string(), JobAction::Provision, now)
        .await
        .map_err(internal(&ctx))?;
    record_event(
        &state,
        &ctx,
        EventAction::ResourceProvisioned,
        EventResult::Accepted,
        "application",
        &app.id.to_string(),
        &format!("imported from {path}"),
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    app = state.apps.get(&app.id).await.map_err(internal(&ctx))?;
    let response = envelope(
        "import",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({
            "application_id": app.id.to_string(),
            "name": name,
            "path": path,
            "environments": app.environments.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        })),
        Some(&job),
        Some("run `labrys inspect --application <id>` to see findings"),
    );
    store_replay(&state, &key, "import", &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

fn internal(ctx: &ActorContext) -> impl Fn(ControlPlaneError) -> ApiError + '_ {
    |err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api.internal",
            redact::redact(&err.to_string(), &[]),
            "retry the request; if it persists, inspect control-plane logs with the trace id",
            &ctx.trace_id,
        )
    }
}

async fn inspect(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let capabilities = state
        .capabilities
        .list_for_application(&app.id)
        .await
        .map_err(internal(&ctx))?;
    let resources = state
        .resources
        .list_for_application(&app.id)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(envelope(
        "inspect",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({
            "application_id": app.id.to_string(),
            "environments": app.environments.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            "capabilities": capabilities.iter().map(|c| c.id.to_string()).collect::<Vec<_>>(),
            "resources": resources.iter().map(|r| r.id.to_string()).collect::<Vec<_>>(),
        })),
        None,
        None,
    )))
}

async fn run_dev(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    mutate_simple(
        &state,
        &headers,
        &raw,
        &body,
        "run",
        JobAction::Provision,
        "dev",
    )
    .await
}

async fn agent(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    mutate_simple(
        &state,
        &headers,
        &raw,
        &body,
        "agent",
        JobAction::Verify,
        "agent",
    )
    .await
}

async fn preview(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    mutate_simple(
        &state,
        &headers,
        &raw,
        &body,
        "preview",
        JobAction::Provision,
        "preview",
    )
    .await
}

async fn capability(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let key = require_idempotency(&ctx, "capability")?;
    if let Some(replayed) = check_replay(&state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let app = load_app(&state, &ctx, &raw).await?;
    let capability = body.capability.unwrap_or_default();
    if capability.trim().is_empty() {
        return Err(ApiError::unprocessable(
            &ctx.trace_id,
            "capability operations require a capability key".to_string(),
            "send {\"capability\": \"database.postgres\"} (secret values stay references only)",
        ));
    }
    let now = chrono::Utc::now();
    let target = format!("{}:capability:{capability}", app.id);
    let (job, _) = enqueue(&state, &target, JobAction::Provision, now)
        .await
        .map_err(internal(&ctx))?;
    record_event(
        &state,
        &ctx,
        EventAction::ResourceProvisioned,
        EventResult::Accepted,
        "capability",
        &capability,
        "capability operation accepted; readiness waits for the platform probe",
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    let response = envelope(
        "capability",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "capability", "id": capability})),
        Some(serde_json::json!({"application_id": app.id.to_string(), "capability": capability})),
        Some(&job),
        Some("capability stays provisioning until the platform observes readiness"),
    );
    store_replay(&state, &key, "capability", &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

async fn mutate_simple(
    state: &ApiState,
    headers: &HeaderMap,
    raw: &str,
    body: &MutationBody,
    command: &'static str,
    action: JobAction,
    target_suffix: &str,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(headers);
    let ctx = actor_context(state, headers, &trace_id)?;
    let key = require_idempotency(&ctx, command)?;
    if let Some(replayed) = check_replay(state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let app = load_app(state, &ctx, raw).await?;
    let now = chrono::Utc::now();
    let target = format!("{}:{target_suffix}", app.id);
    let (job, _) = enqueue(state, &target, action, now)
        .await
        .map_err(internal(&ctx))?;
    // Prompts and session ids shape the event detail by presence only: their
    // content never enters stored evidence, so a pasted secret cannot leak.
    let detail = match command {
        "agent" => format!(
            "agent work requested (prompt {} chars); converging",
            body.prompt.as_deref().unwrap_or("").len()
        ),
        "preview" => format!(
            "preview requested{}; converging",
            body.session_id
                .as_deref()
                .map(|s| format!(" for session {s}"))
                .unwrap_or_default()
        ),
        _ => format!("{command} accepted; converging"),
    };
    record_event(
        state,
        &ctx,
        EventAction::Reconciled,
        EventResult::Accepted,
        "application",
        &app.id.to_string(),
        &detail,
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    let response = envelope(
        command,
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"application_id": app.id.to_string()})),
        Some(&job),
        Some("work continues after this response; poll the job for status"),
    );
    store_replay(state, &key, command, &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

async fn build(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    if let Some(err) = forbid_agent(&ctx, "build") {
        return Err(err);
    }
    let key = require_idempotency(&ctx, "build")?;
    if let Some(replayed) = check_replay(&state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let app = load_app(&state, &ctx, &raw).await?;
    let now = chrono::Utc::now();
    let target = format!("{}:build", app.id);
    let (job, _) = enqueue(&state, &target, JobAction::Provision, now)
        .await
        .map_err(internal(&ctx))?;
    record_event(
        &state,
        &ctx,
        EventAction::Reconciled,
        EventResult::Accepted,
        "application",
        &app.id.to_string(),
        "build accepted; promotion waits for health",
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    let _ = body;
    let response = envelope(
        "build",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"application_id": app.id.to_string()})),
        Some(&job),
        Some("promotion waits for the platform health gate"),
    );
    store_replay(&state, &key, "build", &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

async fn deploy(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    if forbid_agent(&ctx, "deploy").is_some() {
        return Err(ApiError::forbidden(
            &ctx.trace_id,
            "an agent session may not call the production deploy endpoint without approval".to_string(),
            "request human approval first: a named human must approve and run `labrys deploy --environment production --approve-by <name>`",
        ));
    }
    if let Some(err) = require_human(&ctx, "deploy") {
        return Err(err);
    }
    let key = require_idempotency(&ctx, "deploy")?;
    if let Some(replayed) = check_replay(&state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let app = load_app(&state, &ctx, &raw).await?;
    let environment = body
        .environment
        .clone()
        .unwrap_or_else(|| "production".to_string());
    if environment_id_of(&app, &environment).is_none() {
        return Err(ApiError::not_found(
            &ctx.trace_id,
            format!("environment '{environment}' is unknown for this application"),
            "list environments with `labrys inspect --application <id>`",
        ));
    }
    let now = chrono::Utc::now();
    let target = format!("{}:deploy:{environment}", app.id);
    let (job, _) = enqueue(&state, &target, JobAction::Rollout, now)
        .await
        .map_err(internal(&ctx))?;
    let approval_note = body
        .approval
        .as_ref()
        .map(|a| a.approver.clone())
        .unwrap_or_else(|| ctx.actor.kind_label().to_string());
    record_event(
        &state,
        &ctx,
        EventAction::Reconciled,
        EventResult::Accepted,
        "application",
        &app.id.to_string(),
        &format!("deploy to {environment} accepted by {approval_note}; promotion waits for health"),
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    let response = envelope(
        "deploy",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "environment", "id": environment})),
        Some(serde_json::json!({
            "application_id": app.id.to_string(),
            "environment": environment,
            "revision_sha": body.revision_sha,
        })),
        Some(&job),
        Some("deployment is accepted but not healthy until the platform observes health"),
    );
    store_replay(&state, &key, "deploy", &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

async fn rollback(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Json(body): Json<MutationBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    // Agent actors cannot invoke rollback: the command fails here without
    // enqueueing anything, and the output states the approval requirement.
    if forbid_agent(&ctx, "rollback").is_some() {
        return Err(ApiError::forbidden(
            &ctx.trace_id,
            "an agent session may not invoke rollback without named human approval".to_string(),
            "rollback requires a granted, named human approval: a human must run `labrys rollback --approve-by <name> --target <deployment>`",
        ));
    }
    if let Some(err) = require_human(&ctx, "rollback") {
        return Err(err);
    }
    let approval = body
        .approval
        .as_ref()
        .filter(|a| !a.approver.trim().is_empty())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "api.approval_required",
                "rollback requires a granted, named human approval",
                "re-send with {\"approval\": {\"approver\": \"<name>\"}} and --target <deployment>",
                &ctx.trace_id,
            )
        })?;
    let key = require_idempotency(&ctx, "rollback")?;
    if let Some(replayed) = check_replay(&state, Some(&key))
        .await
        .map_err(internal(&ctx))?
    {
        return Ok(Json(replayed));
    }
    let app = load_app(&state, &ctx, &raw).await?;
    let target_id = body.target.clone().ok_or_else(|| {
        ApiError::unprocessable(
            &ctx.trace_id,
            "rollback requires an explicit target deployment".to_string(),
            "list candidates with `labrys deployments --application <id>` and pass --target <id>",
        )
    })?;
    let target_id: DeploymentId = target_id.parse().map_err(|_| {
        ApiError::unprocessable(
            &ctx.trace_id,
            "rollback target is not a valid deployment id".to_string(),
            "list candidates with `labrys deployments --application <id>`",
        )
    })?;
    let target_deployment = state.deployments.get(&target_id).await.map_err(|_| {
        ApiError::not_found(
            &ctx.trace_id,
            format!("rollback target {target_id} does not exist"),
            "list candidates with `labrys deployments --application <id>`",
        )
    })?;
    if target_deployment.image.is_none() {
        return Err(ApiError::unprocessable(
            &ctx.trace_id,
            format!("rollback target {target_id} has no built image"),
            "choose a deployment with a verified OCI artifact",
        ));
    }
    if !matches!(
        target_deployment.phase,
        DeploymentPhase::Healthy | DeploymentPhase::Superseded | DeploymentPhase::RolledBack
    ) {
        return Err(ApiError::unprocessable(
            &ctx.trace_id,
            format!(
                "rollback target {target_id} is {:?}, not a safe target",
                target_deployment.phase
            ),
            "choose a healthy, superseded, or previously rolled-back deployment",
        ));
    }
    let now = chrono::Utc::now();
    let job_target = format!("{}:rollback:{}", app.id, target_id);
    let (job, _) = enqueue(&state, &job_target, JobAction::Rollout, now)
        .await
        .map_err(internal(&ctx))?;
    let approval_record =
        ExplicitApproval::granted(&approval.approver, format!("rollback to {target_id}"));
    record_event(
        &state,
        &ctx,
        EventAction::Reconciled,
        EventResult::Accepted,
        "deployment",
        &target_id.to_string(),
        &format!(
            "rollback to {target_id} approved by {}; {}",
            approval_record.approver,
            labrys_core::DATABASE_DATA_WARNING
        ),
        now,
    )
    .await
    .map_err(internal(&ctx))?;
    let response = envelope(
        "rollback", &ctx, false,
        Some(serde_json::json!({"kind": "deployment", "id": target_id.to_string()})),
        Some(serde_json::json!({
            "application_id": app.id.to_string(),
            "target": target_id.to_string(),
            "approved_by": approval_record.approver,
            "database_data_restored": false,
            "warning": labrys_core::DATABASE_DATA_WARNING,
        })),
        Some(&job),
        Some("traffic moves when the controller converges; database data is never restored by rollback"),
    );
    store_replay(&state, &key, "rollback", &response)
        .await
        .map_err(internal(&ctx))?;
    Ok(Json(response))
}

async fn deployments(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(page): Query<PageQuery>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let (limit, offset) = clamp_page(&page);
    let envs: Vec<_> = match &page.environment {
        Some(name) => app
            .environments
            .iter()
            .filter(|e| &e.name == name)
            .collect(),
        None => app.environments.iter().collect(),
    };
    let mut all = Vec::new();
    for env in envs {
        let list = state
            .deployments
            .list_for_environment(&app.id, &env.id)
            .await
            .map_err(internal(&ctx))?;
        for d in list {
            all.push(serde_json::json!({
                "id": d.id.to_string(),
                "environment": env.name,
                "phase": format!("{:?}", d.phase).to_lowercase(),
                "revision_sha": d.revision.source_sha,
                "digest": d.image.as_ref().map(|i| i.digest.clone()),
                "rollback_target": d.rollback_target.map(|id| id.to_string()),
            }));
        }
    }
    let total = all.len();
    let items: Vec<_> = all.into_iter().skip(offset).take(limit).collect();
    Ok(Json(envelope(
        "deployments",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"total": total, "limit": limit, "offset": offset, "items": items})),
        None,
        None,
    )))
}

async fn logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(page): Query<PageQuery>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let _ = app;
    let (limit, offset) = clamp_page(&page);
    let sources = [
        LogSource::Agent,
        LogSource::Build,
        LogSource::Runtime,
        LogSource::Deployment,
        LogSource::Capability,
        LogSource::Resource,
    ];
    let wanted: Vec<LogSource> = match &page.source {
        Some(name) => sources
            .iter()
            .find(|s| s.canonical_name() == name.as_str())
            .cloned()
            .into_iter()
            .collect(),
        None => sources.to_vec(),
    };
    if page.source.is_some() && wanted.is_empty() {
        return Err(ApiError::unprocessable(
            &ctx.trace_id,
            format!(
                "unknown log source '{}'",
                page.source.as_deref().unwrap_or("")
            ),
            "use one of agent, build, runtime, deployment, capability, resource",
        ));
    }
    let mut entries = Vec::new();
    for source in wanted {
        let list = state
            .logs
            .for_source(source)
            .await
            .map_err(internal(&ctx))?;
        entries.extend(list);
    }
    entries.sort_by_key(|e| e.sequence);
    let total = entries.len();
    let items: Vec<_> = entries
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            serde_json::json!({
                "sequence": e.sequence,
                "source": e.source.canonical_name(),
                "level": format!("{:?}", e.level).to_lowercase(),
                "message": e.message,
                "trace": e.correlation.trace_id,
            })
        })
        .collect();
    Ok(Json(envelope(
        "logs",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": raw})),
        Some(serde_json::json!({"total": total, "limit": limit, "offset": offset, "items": items})),
        None,
        None,
    )))
}

async fn health(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(page): Query<PageQuery>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let envs: Vec<_> = match &page.environment {
        Some(name) => app
            .environments
            .iter()
            .filter(|e| &e.name == name)
            .collect(),
        None => app.environments.iter().collect(),
    };
    let mut entries = Vec::new();
    let mut overall = "unknown";
    for env in envs {
        let list = state
            .deployments
            .list_for_environment(&app.id, &env.id)
            .await
            .map_err(internal(&ctx))?;
        let current = list
            .iter()
            .max_by_key(|d| d.transitions.last().map(|t| t.at));
        match current {
            None => {
                // No deployment yet: report in-progress only when a durable
                // job for this target exists, otherwise unknown — and never
                // healthy without a platform health observation.
                let target = format!("{}:deploy:{}", app.id, env.name);
                let job = state
                    .jobs
                    .latest_for_target(&target)
                    .await
                    .map_err(internal(&ctx))?;
                entries.push(serde_json::json!({
                    "environment": env.name,
                    "state": if job.is_some() { "in_progress" } else { "unknown" },
                    "job": job.as_ref().map(job_value),
                    "detail": "no platform health observation recorded",
                }));
                if job.is_some() && overall == "unknown" {
                    overall = "in_progress";
                }
            }
            Some(d) => {
                let (entry_state, job) = match d.phase {
                    DeploymentPhase::Healthy => ("healthy", None),
                    DeploymentPhase::Building
                    | DeploymentPhase::Deploying
                    | DeploymentPhase::Pending => {
                        let target = format!("{}:deploy:{}", app.id, env.name);
                        let job = state
                            .jobs
                            .latest_for_target(&target)
                            .await
                            .map_err(internal(&ctx))?;
                        ("in_progress", job)
                    }
                    DeploymentPhase::Degraded => ("degraded", None),
                    DeploymentPhase::Failed => ("failed", None),
                    DeploymentPhase::Superseded | DeploymentPhase::RolledBack => {
                        ("superseded", None)
                    }
                };
                entries.push(serde_json::json!({
                    "environment": env.name,
                    "state": entry_state,
                    "deployment": d.id.to_string(),
                    "digest": d.image.as_ref().map(|i| i.digest.clone()),
                    "job": job.as_ref().map(job_value),
                }));
                overall = match (overall, entry_state) {
                    (_, "failed") => "failed",
                    ("failed", _) => "failed",
                    (_, "degraded") => "degraded",
                    ("degraded", _) => "degraded",
                    (_, "in_progress") => "in_progress",
                    ("in_progress", _) => "in_progress",
                    (_, "healthy") => "healthy",
                    (s, _) => s,
                };
            }
        }
    }
    Ok(Json(envelope(
        "health",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"state": overall, "environments": entries})),
        None,
        if overall == "unknown" {
            Some("run `labrys doctor` to see which probe is missing")
        } else {
            None
        },
    )))
}

async fn doctor(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let mut findings = Vec::new();
    let mut has_error = false;
    // Health evidence per environment.
    for env in &app.environments {
        let list = state
            .deployments
            .list_for_environment(&app.id, &env.id)
            .await
            .map_err(internal(&ctx))?;
        let healthy = list.iter().any(|d| d.phase == DeploymentPhase::Healthy);
        if healthy {
            findings.push(serde_json::json!({
                "code": "doctor.health", "severity": "info",
                "message": format!("environment '{}' has a healthy deployment", env.name),
            }));
        } else {
            findings.push(serde_json::json!({
                "code": "doctor.no_health_evidence", "severity": "warning",
                "message": format!("no platform health observation recorded for '{}'", env.name),
                "recovery": "run a health probe or wait for the controller",
            }));
        }
    }
    // Dead-lettered jobs block convergence.
    let dead = state.jobs.dead_letters().await.map_err(internal(&ctx))?;
    if !dead.is_empty() {
        has_error = true;
        findings.push(serde_json::json!({
            "code": "doctor.dead_letters", "severity": "error",
            "message": format!("{} job(s) exhausted retries", dead.len()),
            "recovery": "inspect the job error, fix the cause, then requeue the job",
        }));
    }
    // Failed verification evidence stays visible.
    let evidence = state.evidence.all().await.map_err(internal(&ctx))?;
    for record in evidence.iter().filter(|e| !e.passed) {
        has_error = true;
        findings.push(serde_json::json!({
            "code": "doctor.failed_evidence", "severity": "error",
            "message": record.to_event_detail(),
            "recovery": record.recovery.clone().unwrap_or_else(|| "inspect the failing check and re-run verification".to_string()),
        }));
    }
    // Plugin states.
    let plugins = state.plugins.lock().await;
    for report in plugins.report() {
        match report.state {
            labrys_core::PluginState::Active => findings.push(serde_json::json!({
                "code": "doctor.plugin", "severity": "info",
                "message": format!("{} ({}) active", report.name, report.family.canonical_name()),
            })),
            other => {
                has_error = true;
                findings.push(serde_json::json!({
                    "code": "doctor.plugin_unavailable", "severity": "error",
                    "message": format!("{} ({}) is {other:?}", report.name, report.family.canonical_name()),
                    "recovery": report.detail,
                }));
            }
        }
    }
    drop(plugins);
    Ok(Json(envelope(
        "doctor",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"ok": !has_error, "checks": findings})),
        None,
        None,
    )))
}

async fn capabilities(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let list = state
        .capabilities
        .list_for_application(&app.id)
        .await
        .map_err(internal(&ctx))?;
    let items: Vec<_> = list
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id.to_string(),
                "capability": c.capability,
                "provider": c.provider,
                "mode": format!("{:?}", c.mode).to_lowercase(),
            })
        })
        .collect();
    Ok(Json(envelope(
        "capabilities",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"items": items})),
        None,
        None,
    )))
}

async fn resources(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let app = load_app(&state, &ctx, &raw).await?;
    let list = state
        .resources
        .list_for_application(&app.id)
        .await
        .map_err(internal(&ctx))?;
    let items: Vec<_> = list
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id.to_string(),
                "kind": r.kind,
                "phase": format!("{:?}", r.phase).to_lowercase(),
            })
        })
        .collect();
    Ok(Json(envelope(
        "resources",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "application", "id": app.id.to_string()})),
        Some(serde_json::json!({"items": items})),
        None,
        None,
    )))
}

async fn job_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let id: JobId = raw.parse().map_err(|_| {
        ApiError::not_found(
            &ctx.trace_id,
            format!("job {raw} does not exist"),
            "enqueue work with a mutating command first",
        )
    })?;
    let job = state.jobs.get(&id).await.map_err(|_| {
        ApiError::not_found(
            &ctx.trace_id,
            format!("job {raw} does not exist"),
            "enqueue work with a mutating command first",
        )
    })?;
    let recovery = match job.status {
        labrys_core::JobStatus::Dead => Some("inspect last_error, fix the cause, then requeue"),
        _ => None,
    };
    let mut response = envelope(
        "job",
        &ctx,
        false,
        Some(serde_json::json!({"kind": "job", "id": job.id.to_string()})),
        None,
        Some(&job),
        recovery,
    );
    response["data"] = job
        .last_error
        .clone()
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(response))
}

async fn negotiate_agent(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<NegotiateBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    if let Err(err) = labrys_core::ensure_protocol_version(body.protocol_version) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "api.version_mismatch",
            format!("agent protocol mismatch: {err}"),
            "upgrade the agent backend to the negotiated protocol version before activation",
            &ctx.trace_id,
        ));
    }
    Ok(Json(envelope(
        "negotiate",
        &ctx,
        false,
        None,
        Some(serde_json::json!({
            "family": "agent",
            "negotiated_version": AGENT_PROTOCOL_VERSION,
            "backend": body.backend,
        })),
        None,
        None,
    )))
}

async fn handshake_plugin(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<HandshakeBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let mut plugins = state.plugins.lock().await;
    match plugins.register(body.manifest.clone()) {
        Ok(family) => Ok(Json(envelope(
            "handshake", &ctx, false, None,
            Some(serde_json::json!({
                "family": family.canonical_name(),
                "plugin_id": body.manifest.id,
                "negotiated_version": PLUGIN_PROTOCOL_VERSION,
            })), None, None,
        ))),
        Err(err) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "api.plugin_refused",
            err.to_string(),
            "upgrade the plugin to the negotiated protocol version and implement its family contract",
            &ctx.trace_id,
        )),
    }
}

async fn list_plugins(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let trace_id = trace_of(&headers);
    let ctx = actor_context(&state, &headers, &trace_id)?;
    let plugins = state.plugins.lock().await;
    let items: Vec<_> = plugins
        .report()
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "family": r.family.canonical_name(),
                "state": format!("{:?}", r.state).to_lowercase(),
            })
        })
        .collect();
    drop(plugins);
    Ok(Json(envelope(
        "plugins",
        &ctx,
        false,
        None,
        Some(serde_json::json!({"items": items})),
        None,
        None,
    )))
}

/// Builds the versioned router. The API is a thin layer: handlers validate,
/// enqueue, and report; provider work stays behind controller jobs.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/version", get(version))
        .route("/v1/import", post(import))
        .route("/v1/applications/{app}/inspect", get(inspect))
        .route("/v1/applications/{app}/run", post(run_dev))
        .route("/v1/applications/{app}/agent", post(agent))
        .route("/v1/applications/{app}/preview", post(preview))
        .route(
            "/v1/applications/{app}/capabilities",
            get(capabilities).post(capability),
        )
        .route("/v1/applications/{app}/resources", get(resources))
        .route("/v1/applications/{app}/build", post(build))
        .route("/v1/applications/{app}/deploy", post(deploy))
        .route("/v1/applications/{app}/deployments", get(deployments))
        .route("/v1/applications/{app}/logs", get(logs))
        .route("/v1/applications/{app}/health", get(health))
        .route("/v1/applications/{app}/rollback", post(rollback))
        .route("/v1/applications/{app}/doctor", get(doctor))
        .route("/v1/jobs/{job}", get(job_status))
        .route("/v1/agents/negotiate", post(negotiate_agent))
        .route("/v1/plugins/handshake", post(handshake_plugin))
        .route("/v1/plugins", get(list_plugins))
        .with_state(state)
}
