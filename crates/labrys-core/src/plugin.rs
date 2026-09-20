//! Process-boundary plugin protocol and SDK contracts.
//!
//! Third-party extensions run outside the Rust control plane and speak
//! versioned JSON-RPC over stdin/stdout (the MVP transport). Activation is an
//! explicit handshake: the plugin offers its protocol version and the family
//! contracts it implements, and the control plane either accepts it or refuses
//! it with a structured error. An incompatible or unavailable plugin can never
//! crash or panic the control plane, calls carry explicit cancellation, and
//! every failure surfaces as a JSON-RPC error object rather than a transport
//! break.
//!
//! The six extension families are `AgentProvider`, `CapabilityProvider`,
//! `BindingProvider`, `RuntimeProvider`, `InspectorProvider`, and
//! `DeployProvider`. `sdk` holds one working example plugin per family.
//!
//! This module is pure and deterministic: the framed transport is modeled by
//! [`PluginProcess`] message exchange, not real pipes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{CoreError, Result};

/// Protocol version the control plane speaks. A plugin offering anything else
/// is refused at handshake.
pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;

/// Extension family a plugin belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginFamily {
    AgentProvider,
    CapabilityProvider,
    BindingProvider,
    RuntimeProvider,
    InspectorProvider,
    DeployProvider,
}

impl PluginFamily {
    /// Every extension family.
    pub fn all() -> &'static [PluginFamily] {
        &[
            Self::AgentProvider,
            Self::CapabilityProvider,
            Self::BindingProvider,
            Self::RuntimeProvider,
            Self::InspectorProvider,
            Self::DeployProvider,
        ]
    }

    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::AgentProvider => "agent_provider",
            Self::CapabilityProvider => "capability_provider",
            Self::BindingProvider => "binding_provider",
            Self::RuntimeProvider => "runtime_provider",
            Self::InspectorProvider => "inspector_provider",
            Self::DeployProvider => "deploy_provider",
        }
    }

    /// Methods the family contract requires a plugin to answer.
    pub fn required_methods(self) -> &'static [&'static str] {
        match self {
            Self::AgentProvider => &["agent.draft_events", "agent.interrupt"],
            Self::CapabilityProvider => &["capability.plan", "capability.observe"],
            Self::BindingProvider => &["binding.resolve"],
            Self::RuntimeProvider => &["runtime.detect", "runtime.prepare"],
            Self::InspectorProvider => &["inspect.snapshot", "inspect.findings"],
            Self::DeployProvider => &["deploy.build", "deploy.rollback"],
        }
    }
}

/// What a plugin declared itself able to do at handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    /// Plugin's own semantic version; informational.
    pub version: String,
    pub family: PluginFamily,
    /// Protocol version the plugin offers.
    pub protocol_version: u32,
    /// Method names the plugin answers.
    #[serde(default)]
    pub methods: Vec<String>,
    /// Launch command crossing the process boundary (stdin/stdout framed).
    #[serde(default)]
    pub command: Vec<String>,
    /// Free-form capability hints surfaced in `doctor` and the dashboard.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl PluginManifest {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        family: PluginFamily,
        protocol_version: u32,
        methods: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: "0.1.0".to_string(),
            family,
            protocol_version,
            methods,
            command: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_command(mut self, command: Vec<String>) -> Self {
        self.command = command;
        self
    }

    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// True when the manifest answers every method its family contract needs.
    pub fn implements_family(&self) -> bool {
        self.family
            .required_methods()
            .iter()
            .all(|method| self.methods.iter().any(|m| m == method))
    }
}

/// JSON-RPC error codes reserved by this protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RpcErrorCode {
    /// Malformed frame.
    ParseError,
    /// Not a valid request object.
    InvalidRequest,
    /// Plugin does not answer the method.
    MethodNotFound,
    /// Params failed validation.
    InvalidParams,
    /// Plugin-internal failure.
    InternalError,
    /// Plugin process is not running.
    PluginUnavailable,
    /// Call was cancelled by the control plane.
    Cancelled,
    /// Handshake protocol version mismatch.
    VersionMismatch,
}

impl RpcErrorCode {
    /// Stable numeric code (JSON-RPC reserved range for protocol errors).
    pub fn code(self) -> i32 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
            Self::PluginUnavailable => -32000,
            Self::Cancelled => -32800,
            Self::VersionMismatch => -32001,
        }
    }
}

impl std::fmt::Display for RpcErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({})",
            self.code(),
            match self {
                Self::ParseError => "parse_error",
                Self::InvalidRequest => "invalid_request",
                Self::MethodNotFound => "method_not_found",
                Self::InvalidParams => "invalid_params",
                Self::InternalError => "internal_error",
                Self::PluginUnavailable => "plugin_unavailable",
                Self::Cancelled => "cancelled",
                Self::VersionMismatch => "version_mismatch",
            }
        )
    }
}

/// Error object carried by a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
    /// Actionable recovery hint; never contains secret values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

impl RpcError {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            recovery: None,
        }
    }

    pub fn with_recovery(mut self, recovery: impl Into<String>) -> Self {
        self.recovery = Some(recovery.into());
        self
    }

    /// Maps a [`CoreError`] to the wire error a plugin should return.
    pub fn from_core_error(err: &CoreError) -> Self {
        match err {
            CoreError::PluginUnavailable(message) => {
                Self::new(RpcErrorCode::PluginUnavailable, message.clone())
            }
            CoreError::Plugin(message) => Self::new(RpcErrorCode::InvalidParams, message.clone()),
            CoreError::ProtocolVersion { found, expected } => Self::new(
                RpcErrorCode::VersionMismatch,
                format!("offered protocol v{found}, control plane speaks v{expected}"),
            ),
            other => Self::new(RpcErrorCode::InternalError, other.to_string()),
        }
    }
}

/// One JSON-RPC request crossing the boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl RpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }
}

/// One JSON-RPC response. Exactly one of `result` / `error` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcResponse {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: u64, error: RpcError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }
}

/// Result of the activation handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handshake {
    pub plugin_id: String,
    pub family: PluginFamily,
    /// Version both sides agreed on.
    pub negotiated_version: u32,
    pub methods: Vec<String>,
    pub capabilities: Vec<String>,
}

/// Lifecycle of a connected plugin process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginState {
    /// Handshake succeeded; methods may be invoked.
    Active,
    /// Refused at handshake (incompatible). Never invoked.
    Refused,
    /// Was active; the process stopped answering.
    Unavailable,
    /// Explicitly shut down.
    Stopped,
}

/// Control-plane handle for one plugin process.
///
/// Connection performs the handshake; an incompatible or missing plugin is
/// recorded as [`PluginState::Refused`] / [`PluginState::Unavailable`] and
/// every later call returns a structured error instead of panicking, so one
/// bad third-party process cannot take the control plane down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginProcess {
    manifest: PluginManifest,
    state: PluginState,
    handshake: Option<Handshake>,
    refusal: Option<RpcError>,
    next_call_id: u64,
    in_flight: BTreeMap<u64, String>,
    cancelled: Vec<u64>,
    calls: Vec<(String, u64)>,
}

impl PluginProcess {
    /// Performs the handshake for `manifest`.
    pub fn connect(manifest: PluginManifest) -> Self {
        let refusal = match Self::negotiate(&manifest) {
            Ok(handshake) => {
                return Self {
                    manifest,
                    state: PluginState::Active,
                    handshake: Some(handshake),
                    refusal: None,
                    next_call_id: 1,
                    in_flight: BTreeMap::new(),
                    cancelled: Vec::new(),
                    calls: Vec::new(),
                }
            }
            Err(error) => RpcError::from_core_error(&error),
        };
        Self {
            manifest,
            state: PluginState::Refused,
            handshake: None,
            refusal: Some(refusal),
            next_call_id: 1,
            in_flight: BTreeMap::new(),
            cancelled: Vec::new(),
            calls: Vec::new(),
        }
    }

    /// Version negotiation and family contract check.
    fn negotiate(manifest: &PluginManifest) -> Result<Handshake> {
        if manifest.protocol_version != PLUGIN_PROTOCOL_VERSION {
            return Err(CoreError::ProtocolVersion {
                found: manifest.protocol_version,
                expected: PLUGIN_PROTOCOL_VERSION,
            });
        }
        if manifest.name.trim().is_empty() {
            return Err(CoreError::Plugin(
                "plugin manifest must name the plugin".to_string(),
            ));
        }
        if !manifest.implements_family() {
            let missing: Vec<&str> = manifest
                .family
                .required_methods()
                .iter()
                .copied()
                .filter(|method| !manifest.methods.iter().any(|m| m == method))
                .collect();
            return Err(CoreError::Plugin(format!(
                "plugin '{}' does not implement its {} contract, missing {:?}",
                manifest.name,
                manifest.family.canonical_name(),
                missing
            )));
        }
        Ok(Handshake {
            plugin_id: manifest.id.clone(),
            family: manifest.family,
            negotiated_version: PLUGIN_PROTOCOL_VERSION,
            methods: manifest.methods.clone(),
            capabilities: manifest.capabilities.clone(),
        })
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn state(&self) -> PluginState {
        self.state
    }

    pub fn is_active(&self) -> bool {
        self.state == PluginState::Active
    }

    pub fn handshake(&self) -> Option<&Handshake> {
        self.handshake.as_ref()
    }

    /// Structured reason a plugin was refused, if any.
    pub fn refusal(&self) -> Option<&RpcError> {
        self.refusal.as_ref()
    }

    pub fn family(&self) -> PluginFamily {
        self.manifest.family
    }

    /// Methods the plugin may be called with.
    pub fn methods(&self) -> &[String] {
        &self.manifest.methods
    }

    /// Number of calls that reached the plugin.
    pub fn call_count(&self) -> usize {
        self.calls.len()
    }

    /// Marks the plugin process dead; later calls report unavailability.
    pub fn mark_unavailable(&mut self) {
        if self.state == PluginState::Active {
            self.state = PluginState::Unavailable;
            self.in_flight.clear();
        }
    }

    /// Explicitly shuts the plugin down.
    pub fn shutdown(&mut self) {
        if self.state != PluginState::Refused {
            self.state = PluginState::Stopped;
            self.in_flight.clear();
        }
    }

    /// Invokes a method, returning the response the plugin sent back.
    ///
    /// Refused, unavailable, and stopped plugins answer with an error object;
    /// unknown methods answer `method_not_found`; a plugin handler that fails
    /// answers `internal_error`. None of these abort the control plane.
    pub fn invoke<F>(&mut self, method: &str, params: Value, handler: F) -> RpcResponse
    where
        F: FnOnce(&str, Value) -> Result<Value>,
    {
        if self.state != PluginState::Active {
            return RpcResponse::err(self.next_call_id, self.unavailable_error(method));
        }
        if !self.manifest.methods.iter().any(|m| m == method) {
            let id = self.next_call_id;
            self.next_call_id += 1;
            return RpcResponse::err(
                id,
                RpcError::new(
                    RpcErrorCode::MethodNotFound,
                    format!("plugin '{}' does not answer '{method}'", self.manifest.name),
                )
                .with_recovery("check the plugin manifest methods"),
            );
        }
        let id = self.next_call_id;
        self.next_call_id += 1;
        self.in_flight.insert(id, method.to_string());
        let response = match handler(method, params) {
            Ok(result) => RpcResponse::ok(id, result),
            Err(err) => RpcResponse::err(id, RpcError::from_core_error(&err)),
        };
        self.in_flight.remove(&id);
        self.calls.push((method.to_string(), id));
        response
    }

    /// Id the next call will receive.
    pub fn next_pending_id(&self) -> u64 {
        self.next_call_id
    }

    /// Registers a call as in flight without dispatching it, so the control
    /// plane can cancel a long-running plugin call.
    pub fn begin_call(&mut self, id: u64, method: &str) {
        self.in_flight.insert(id, method.to_string());
        self.next_call_id = self.next_call_id.max(id + 1);
    }

    /// Cancels an in-flight call by id. A cancelled call reports
    /// [`RpcErrorCode::Cancelled`] rather than hanging the control plane.
    pub fn cancel(&mut self, id: u64) -> Result<()> {
        let method = self
            .in_flight
            .remove(&id)
            .ok_or_else(|| CoreError::Plugin(format!("no in-flight call {id} to cancel")))?;
        self.cancelled.push(id);
        self.calls.push((format!("{method}#cancelled"), id));
        Ok(())
    }

    /// Response for a call that was cancelled while in flight.
    pub fn cancelled_response(&self, id: u64) -> Option<RpcResponse> {
        if self.cancelled.contains(&id) {
            return Some(RpcResponse::err(
                id,
                RpcError::new(RpcErrorCode::Cancelled, "call cancelled by control plane")
                    .with_recovery("retry with a longer timeout or a smaller request"),
            ));
        }
        None
    }

    pub fn cancelled(&self) -> &[u64] {
        &self.cancelled
    }

    fn unavailable_error(&self, method: &str) -> RpcError {
        let code = match self.state {
            PluginState::Unavailable => RpcErrorCode::PluginUnavailable,
            PluginState::Refused => RpcErrorCode::VersionMismatch,
            PluginState::Stopped => RpcErrorCode::PluginUnavailable,
            PluginState::Active => RpcErrorCode::InternalError,
        };
        let message = match self.state {
            PluginState::Refused => self
                .refusal
                .as_ref()
                .map(|r| r.message.clone())
                .unwrap_or_else(|| "plugin refused at handshake".to_string()),
            other => format!(
                "plugin '{}' is {other:?}, cannot invoke '{method}'",
                self.manifest.name
            ),
        };
        RpcError::new(code, message).with_recovery(match self.state {
            PluginState::Refused => "upgrade the plugin to protocol v{PLUGIN_PROTOCOL_VERSION}",
            _ => "restart the plugin process or disable it",
        })
    }
}

/// Registry of plugins by family, one active plugin per family in MVP.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, PluginProcess>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Connects and registers a plugin. An incompatible plugin is still
    /// recorded (as refused) so `doctor` can report it, and registration
    /// returns the structured error.
    pub fn register(&mut self, manifest: PluginManifest) -> Result<PluginFamily> {
        let process = PluginProcess::connect(manifest.clone());
        let family = manifest.family;
        if let Some(error) = process.refusal() {
            let message = error.message.clone();
            self.plugins.insert(manifest.id, process);
            return Err(CoreError::Plugin(message));
        }
        if let Some(existing) = self.plugins.values().find(|p| {
            p.family() == family
                && p.state() == PluginState::Active
                && p.manifest().id != manifest.id
        }) {
            let name = existing.manifest().name.clone();
            return Err(CoreError::Plugin(format!(
                "family {} already has an active plugin '{name}'",
                family.canonical_name()
            )));
        }
        self.plugins.insert(manifest.id, process);
        Ok(family)
    }

    pub fn get(&self, id: &str) -> Option<&PluginProcess> {
        self.plugins.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut PluginProcess> {
        self.plugins.get_mut(id)
    }

    /// The active plugin for a family, if any.
    pub fn active_for(&self, family: PluginFamily) -> Option<&PluginProcess> {
        self.plugins
            .values()
            .find(|p| p.family() == family && p.state() == PluginState::Active)
    }

    pub fn active_for_mut(&mut self, family: PluginFamily) -> Option<&mut PluginProcess> {
        self.plugins
            .values_mut()
            .find(|p| p.family() == family && p.state() == PluginState::Active)
    }

    pub fn plugins(&self) -> impl Iterator<Item = &PluginProcess> {
        self.plugins.values()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Structured plugin health for `doctor` and the dashboard.
    pub fn report(&self) -> Vec<PluginReport> {
        self.plugins
            .values()
            .map(|plugin| PluginReport {
                id: plugin.manifest().id.clone(),
                name: plugin.manifest().name.clone(),
                family: plugin.family(),
                state: plugin.state(),
                protocol_version: plugin.manifest().protocol_version,
                detail: plugin
                    .refusal()
                    .map(|r| format!("{}: {}", r.code, r.message))
                    .unwrap_or_else(|| format!("{} methods", plugin.methods().len())),
            })
            .collect()
    }
}

/// One plugin's registration status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginReport {
    pub id: String,
    pub name: String,
    pub family: PluginFamily,
    pub state: PluginState,
    pub protocol_version: u32,
    pub detail: String,
}

impl PluginReport {
    pub fn to_event_detail(&self) -> String {
        format!(
            "plugin {} ({}) state {:?} protocol v{}: {}",
            self.name,
            self.family.canonical_name(),
            self.state,
            self.protocol_version,
            self.detail
        )
    }
}

/// Builds the JSON frame a plugin writes to stdout.
pub fn encode_response(response: &RpcResponse) -> Result<String> {
    let mut frame = json!({ "jsonrpc": "2.0", "id": response.id });
    match (&response.result, &response.error) {
        (Some(result), None) => frame["result"] = result.clone(),
        (None, Some(error)) => {
            frame["error"] = serde_json::to_value(error).map_err(CoreError::from)?
        }
        _ => {
            return Err(CoreError::Plugin(
                "response must carry exactly one of result or error".to_string(),
            ))
        }
    }
    serde_json::to_string(&frame).map_err(CoreError::from)
}

/// Parses a JSON frame received from a plugin.
pub fn decode_response(frame: &str) -> Result<RpcResponse> {
    let value: Value = serde_json::from_str(frame)
        .map_err(|_| CoreError::Plugin("malformed JSON-RPC frame from plugin".to_string()))?;
    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| CoreError::Plugin("JSON-RPC frame is missing an id".to_string()))?;
    if let Some(error) = value.get("error") {
        let parsed: RpcError = serde_json::from_value(error.clone()).map_err(CoreError::from)?;
        return Ok(RpcResponse::err(id, parsed));
    }
    let result = value.get("result").cloned().ok_or_else(|| {
        CoreError::Plugin("JSON-RPC frame carries neither result nor error".to_string())
    })?;
    Ok(RpcResponse::ok(id, result))
}

// ---------------------------------------------------------------------------
// SDK: one example plugin per extension family
// ---------------------------------------------------------------------------

/// Contract a plugin implementation satisfies inside its own process. The
/// control plane only ever sees the manifest and JSON-RPC frames.
pub trait Plugin {
    fn manifest(&self) -> PluginManifest;

    /// Answers one method. Errors become JSON-RPC error objects.
    fn handle(&mut self, method: &str, params: Value) -> Result<Value>;

    /// Convenience: run one request through the handler.
    fn respond(&mut self, request: &RpcRequest) -> RpcResponse {
        let manifest = self.manifest();
        if !manifest.methods.iter().any(|m| m == &request.method) {
            return RpcResponse::err(
                request.id,
                RpcError::new(
                    RpcErrorCode::MethodNotFound,
                    format!("'{}' is not implemented", request.method),
                ),
            );
        }
        match self.handle(&request.method, request.params.clone()) {
            Ok(result) => RpcResponse::ok(request.id, result),
            Err(err) => RpcResponse::err(request.id, RpcError::from_core_error(&err)),
        }
    }
}

/// Example `AgentProvider`: normalizes a prompt into canonical event kinds.
pub struct ExampleAgentPlugin {
    name: String,
}

impl ExampleAgentPlugin {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Plugin for ExampleAgentPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-agent",
            self.name.clone(),
            PluginFamily::AgentProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec![
                "agent.draft_events".to_string(),
                "agent.interrupt".to_string(),
            ],
        )
        .with_command(vec!["node".to_string(), "agent-plugin.js".to_string()])
        .with_capabilities(vec!["prompt".to_string(), "interrupt".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "agent.draft_events" => {
                let prompt = params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CoreError::Plugin("agent.draft_events needs 'prompt'".to_string())
                    })?;
                Ok(json!({
                    "events": [
                        { "kind": "planning", "detail": format!("{} planned: {prompt}", self.name) },
                        { "kind": "command_run", "detail": format!("{} acted", self.name) },
                    ]
                }))
            }
            "agent.interrupt" => Ok(json!({ "interrupted": true })),
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// Example `InspectorProvider`: reports findings for a file listing.
pub struct ExampleInspectorPlugin;

impl Plugin for ExampleInspectorPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-inspector",
            "docker-compose-inspector",
            PluginFamily::InspectorProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec![
                "inspect.snapshot".to_string(),
                "inspect.findings".to_string(),
            ],
        )
        .with_command(vec!["bun".to_string(), "inspector-plugin.ts".to_string()])
        .with_capabilities(vec!["compose".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "inspect.snapshot" => {
                let files = params
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                Ok(json!({ "files_seen": files }))
            }
            "inspect.findings" => {
                let has_compose = params
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .any(|item| item.as_str().unwrap_or_default().ends_with("compose.yaml"))
                    })
                    .unwrap_or(false);
                Ok(json!({
                    "findings": has_compose.then(|| json!({
                        "name": "compose",
                        "value": "present",
                        "confidence": 1.0,
                    }))
                }))
            }
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// Example `RuntimeProvider`: claims any project with a Dockerfile.
pub struct ExampleRuntimePlugin;

impl Plugin for ExampleRuntimePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-runtime",
            "haskell-ghcup-runtime",
            PluginFamily::RuntimeProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec!["runtime.detect".to_string(), "runtime.prepare".to_string()],
        )
        .with_command(vec!["node".to_string(), "runtime-plugin.js".to_string()])
        .with_capabilities(vec!["oci".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "runtime.detect" => {
                let has_dockerfile = params
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .any(|item| item.as_str().unwrap_or_default().ends_with("Dockerfile"))
                    })
                    .unwrap_or(false);
                Ok(json!({
                    "supported": has_dockerfile,
                    "kind": has_dockerfile.then_some("generic"),
                    "tier": has_dockerfile.then_some("tier0"),
                }))
            }
            "runtime.prepare" => Ok(json!({
                "run_command": ["./entrypoint"],
                "oci_image": "registry.local/plugin-app",
                "profile": "production",
            })),
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// Example `CapabilityProvider`: plans a managed Postgres resource.
pub struct ExampleCapabilityProviderPlugin;

impl Plugin for ExampleCapabilityProviderPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-capability",
            "neon-postgres",
            PluginFamily::CapabilityProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec![
                "capability.plan".to_string(),
                "capability.observe".to_string(),
            ],
        )
        .with_command(vec!["node".to_string(), "capability-plugin.js".to_string()])
        .with_capabilities(vec!["database.postgres".to_string(), "managed".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "capability.plan" => {
                let capability = params
                    .get("capability")
                    .and_then(Value::as_str)
                    .unwrap_or("database.postgres");
                Ok(json!({
                    "capability": capability,
                    "mode": "managed",
                    "resource": { "external_ref": format!("{capability}-plan") },
                    // Only a reference, never a value.
                    "secret_refs": [format!("neon.{capability}.url")],
                }))
            }
            "capability.observe" => Ok(json!({ "phase": "ready" })),
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// Example `BindingProvider`: resolves a portable `DATABASE_URL` binding.
pub struct ExampleBindingProviderPlugin;

impl Plugin for ExampleBindingProviderPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-binding",
            "env-contract-binding",
            PluginFamily::BindingProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec!["binding.resolve".to_string()],
        )
        .with_command(vec!["node".to_string(), "binding-plugin.js".to_string()])
        .with_capabilities(vec!["generic".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "binding.resolve" => {
                let provider = params
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("postgres");
                Ok(json!({
                    "kind": "generic",
                    "env": { "DATABASE_URL": { "secret_ref": format!("{provider}.database_url") } },
                }))
            }
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// Example `DeployProvider`: builds an OCI artifact and rolls back by digest.
pub struct ExampleDeployPlugin;

impl Plugin for ExampleDeployPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            "example-deploy",
            "fly-deploy",
            PluginFamily::DeployProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec!["deploy.build".to_string(), "deploy.rollback".to_string()],
        )
        .with_command(vec!["node".to_string(), "deploy-plugin.js".to_string()])
        .with_capabilities(vec!["oci".to_string(), "rollback".to_string()])
    }

    fn handle(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "deploy.build" => {
                let revision = params
                    .get("revision")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CoreError::Plugin("deploy.build needs 'revision'".to_string())
                    })?;
                Ok(json!({ "digest": format!("sha256:{revision}"), "health_gate": true }))
            }
            "deploy.rollback" => {
                let digest = params
                    .get("digest")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CoreError::Plugin("deploy.rollback needs 'digest'".to_string())
                    })?;
                Ok(json!({
                    "rolled_back_to": digest,
                    "traffic_moved": true,
                    "database_data_restored": false,
                    "warning": crate::deployment::DATABASE_DATA_WARNING,
                }))
            }
            other => Err(CoreError::Plugin(format!("unhandled method {other}"))),
        }
    }
}

/// One example plugin per family, ready for `doctor` and SDK documentation.
pub fn sdk_examples() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(ExampleAgentPlugin::new("opencode")),
        Box::new(ExampleCapabilityProviderPlugin),
        Box::new(ExampleBindingProviderPlugin),
        Box::new(ExampleRuntimePlugin),
        Box::new(ExampleInspectorPlugin),
        Box::new(ExampleDeployPlugin),
    ]
}

/// Registers every SDK example plugin and returns the family each joined.
pub fn register_sdk_examples(registry: &mut PluginRegistry) -> Result<Vec<PluginFamily>> {
    let mut families = Vec::new();
    for example in sdk_examples() {
        families.push(registry.register(example.manifest())?);
    }
    Ok(families)
}
