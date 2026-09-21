//! Typed client for the control-plane API, shared by the `labrys` CLI.
//!
//! One client carries the base URL, bearer token, actor identity, and trace
//! through every command, so the CLI cannot accidentally drop authentication,
//! attribution, or idempotency. All responses are machine-readable JSON
//! envelopes; failures carry the recovery path the server returned.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::api::API_VERSION;

/// Exit semantics for CLI commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Command succeeded.
    Ok = 0,
    /// Command ran but reported failure (denied, unprocessable, failed state).
    CommandFailed = 1,
    /// Usage, authentication, connection, or protocol error.
    Usage = 2,
}

/// Typed control-plane client over HTTP.
#[derive(Debug, Clone)]
pub struct ControlPlaneClient {
    base_url: String,
    token: String,
    actor: String,
    trace_id: String,
    http: reqwest::Client,
}

impl ControlPlaneClient {
    pub fn new(
        base_url: impl Into<String>,
        token: impl Into<String>,
        actor: impl Into<String>,
    ) -> Result<Self, String> {
        Self::with_trace(
            base_url,
            token,
            actor,
            format!("trace-{}", uuid::Uuid::new_v4().simple()),
        )
    }

    pub fn with_trace(
        base_url: impl Into<String>,
        token: impl Into<String>,
        actor: impl Into<String>,
        trace_id: impl Into<String>,
    ) -> Result<Self, String> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err("API base URL is required (--api-url or LABRYS_API_URL)".to_string());
        }
        let token = token.into();
        if token.is_empty() {
            return Err("API token is required (--token or LABRYS_API_TOKEN)".to_string());
        }
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("cannot build HTTP client: {e}"))?;
        Ok(Self {
            base_url,
            token,
            actor: actor.into(),
            trace_id: trace_id.into(),
            http,
        })
    }

    pub fn api_version() -> u32 {
        API_VERSION
    }

    fn headers(&self, idempotency_key: Option<&str>) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.token)
                .parse()
                .expect("token is header-safe"),
        );
        headers.insert(
            "x-labryst-actor",
            self.actor.parse().expect("actor is header-safe"),
        );
        headers.insert(
            "x-trace-id",
            self.trace_id.parse().expect("trace is header-safe"),
        );
        if let Some(key) = idempotency_key {
            headers.insert("Idempotency-Key", key.parse().expect("key is header-safe"));
        }
        headers
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .get(&url)
            .query(query)
            .headers(self.headers(None))
            .send()
            .await
            .map_err(|e| format!("request to {path} failed: {e}"))?;
        response
            .json::<Value>()
            .await
            .map_err(|e| format!("response from {path} is not JSON: {e}"))
    }

    async fn post(
        &self,
        path: &str,
        idempotency_key: Option<&str>,
        body: &Value,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .post(&url)
            .headers(self.headers(idempotency_key))
            .json(body)
            .send()
            .await
            .map_err(|e| format!("request to {path} failed: {e}"))?;
        response
            .json::<Value>()
            .await
            .map_err(|e| format!("response from {path} is not JSON: {e}"))
    }

    pub async fn version(&self) -> Result<Value, String> {
        let url = format!("{}/v1/version", self.base_url);
        self.http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("version request failed: {e}"))?
            .json::<Value>()
            .await
            .map_err(|e| format!("version response is not JSON: {e}"))
    }

    pub async fn import(
        &self,
        name: Option<&str>,
        path: Option<&str>,
        idempotency_key: &str,
    ) -> Result<Value, String> {
        self.post(
            "/v1/import",
            Some(idempotency_key),
            &serde_json::json!({ "name": name, "path": path }),
        )
        .await
    }

    pub async fn inspect(&self, application: &str) -> Result<Value, String> {
        self.get(&format!("/v1/applications/{application}/inspect"), &[])
            .await
    }

    async fn mutate(
        &self,
        application: &str,
        command: &str,
        idempotency_key: &str,
        body: &BTreeMap<String, String>,
        approval: Option<&str>,
    ) -> Result<Value, String> {
        let mut payload = serde_json::Map::new();
        for (key, value) in body {
            payload.insert(key.clone(), Value::String(value.clone()));
        }
        if let Some(approver) = approval {
            payload.insert(
                "approval".to_string(),
                serde_json::json!({ "approver": approver }),
            );
        }
        self.post(
            &format!("/v1/applications/{application}/{command}"),
            Some(idempotency_key),
            &Value::Object(payload),
        )
        .await
    }

    pub async fn run(
        &self,
        application: &str,
        idempotency_key: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<Value, String> {
        self.mutate(application, "run", idempotency_key, args, None)
            .await
    }

    pub async fn agent(
        &self,
        application: &str,
        idempotency_key: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<Value, String> {
        self.mutate(application, "agent", idempotency_key, args, None)
            .await
    }

    pub async fn preview(
        &self,
        application: &str,
        idempotency_key: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<Value, String> {
        self.mutate(application, "preview", idempotency_key, args, None)
            .await
    }

    pub async fn capability(
        &self,
        application: &str,
        capability: &str,
        idempotency_key: &str,
    ) -> Result<Value, String> {
        let mut args = BTreeMap::new();
        args.insert("capability".to_string(), capability.to_string());
        self.mutate(application, "capabilities", idempotency_key, &args, None)
            .await
    }

    pub async fn capabilities(&self, application: &str) -> Result<Value, String> {
        self.get(&format!("/v1/applications/{application}/capabilities"), &[])
            .await
    }

    pub async fn resources(&self, application: &str) -> Result<Value, String> {
        self.get(&format!("/v1/applications/{application}/resources"), &[])
            .await
    }

    pub async fn build(
        &self,
        application: &str,
        idempotency_key: &str,
        args: &BTreeMap<String, String>,
    ) -> Result<Value, String> {
        self.mutate(application, "build", idempotency_key, args, None)
            .await
    }

    pub async fn deploy(
        &self,
        application: &str,
        environment: &str,
        idempotency_key: &str,
        approval: Option<&str>,
    ) -> Result<Value, String> {
        let mut args = BTreeMap::new();
        args.insert("environment".to_string(), environment.to_string());
        self.mutate(application, "deploy", idempotency_key, &args, approval)
            .await
    }

    pub async fn deployments(
        &self,
        application: &str,
        environment: Option<&str>,
        limit: i64,
    ) -> Result<Value, String> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(env) = environment {
            query.push(("environment", env.to_string()));
        }
        self.get(
            &format!("/v1/applications/{application}/deployments"),
            &query,
        )
        .await
    }

    pub async fn logs(
        &self,
        application: &str,
        source: Option<&str>,
        limit: i64,
    ) -> Result<Value, String> {
        let mut query = vec![("limit", limit.to_string())];
        if let Some(source) = source {
            query.push(("source", source.to_string()));
        }
        self.get(&format!("/v1/applications/{application}/logs"), &query)
            .await
    }

    pub async fn health(
        &self,
        application: &str,
        environment: Option<&str>,
    ) -> Result<Value, String> {
        let query: Vec<(&str, String)> = environment
            .map(|env| vec![("environment", env.to_string())])
            .unwrap_or_default();
        self.get(&format!("/v1/applications/{application}/health"), &query)
            .await
    }

    pub async fn rollback(
        &self,
        application: &str,
        target: &str,
        approver: &str,
        idempotency_key: &str,
    ) -> Result<Value, String> {
        let mut args = BTreeMap::new();
        args.insert("target".to_string(), target.to_string());
        self.mutate(
            application,
            "rollback",
            idempotency_key,
            &args,
            Some(approver),
        )
        .await
    }

    pub async fn doctor(&self, application: &str) -> Result<Value, String> {
        self.get(&format!("/v1/applications/{application}/doctor"), &[])
            .await
    }

    pub async fn job(&self, job: &str) -> Result<Value, String> {
        self.get(&format!("/v1/jobs/{job}"), &[]).await
    }

    pub async fn negotiate_agent(
        &self,
        protocol_version: u32,
        backend: Option<&str>,
    ) -> Result<Value, String> {
        self.post(
            "/v1/agents/negotiate",
            None,
            &serde_json::json!({ "protocol_version": protocol_version, "backend": backend }),
        )
        .await
    }

    pub async fn handshake_plugin(&self, manifest: &Value) -> Result<Value, String> {
        self.post(
            "/v1/plugins/handshake",
            None,
            &serde_json::json!({ "manifest": manifest }),
        )
        .await
    }

    pub async fn plugins(&self) -> Result<Value, String> {
        self.get("/v1/plugins", &[]).await
    }
}

/// Maps an API envelope to a process exit code: transport/usage errors are
/// `Usage`, command failures are `CommandFailed`, successes are `Ok`.
pub fn exit_for(envelope: &Value) -> ExitCode {
    match envelope.get("ok").and_then(Value::as_bool) {
        Some(true) => ExitCode::Ok,
        _ => ExitCode::CommandFailed,
    }
}

/// Renders the actionable recovery from an envelope for terminal output.
pub fn recovery_of(envelope: &Value) -> Option<String> {
    envelope
        .get("recovery")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            envelope
                .pointer("/data/recovery")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}
