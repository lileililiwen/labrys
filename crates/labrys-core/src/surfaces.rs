//! CLI command contracts and dashboard read/write boundaries.
//!
//! The CLI is a stable surface over the same platform objects the control
//! plane uses: every command returns a machine-readable envelope carrying
//! structured findings tied to the application, and every failure carries
//! actionable recovery information. Mutating commands require an idempotency
//! key, so a retried invocation replays the recorded result instead of
//! repeating the side effect, and each actor role is checked against the
//! command before anything runs.
//!
//! The dashboard keeps agent output and platform output in separate streams:
//! an agent's completion claim is displayed as an agent-attributed entry and
//! never as platform health. Production health is computed only from
//! platform-owned evidence, so an agent reporting success cannot make a failed
//! deployment look healthy. Secrets are exposed as references only.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};
use crate::ids::ApplicationId;
use crate::inspector::ExplicitApproval;
use crate::observability::{
    AuditLog, EventLog, EvidenceStore, Finding, HealthSnapshot, LogSource, LogStore, Severity,
};
use crate::plugin::PluginRegistry;

// ---------------------------------------------------------------------------
// CLI command contracts
// ---------------------------------------------------------------------------

/// Stable CLI command vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum CliCommand {
    Init,
    Import,
    Inspect,
    Dev,
    Agent,
    Preview,
    Capability,
    Resources,
    Build,
    Deploy,
    Deployments,
    Logs,
    Health,
    Rollback,
    Doctor,
}

impl CliCommand {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Import => "import",
            Self::Inspect => "inspect",
            Self::Dev => "dev",
            Self::Agent => "agent",
            Self::Preview => "preview",
            Self::Capability => "capability",
            Self::Resources => "resources",
            Self::Build => "build",
            Self::Deploy => "deploy",
            Self::Deployments => "deployments",
            Self::Logs => "logs",
            Self::Health => "health",
            Self::Rollback => "rollback",
            Self::Doctor => "doctor",
        }
    }

    /// Mutating commands change platform state and require an idempotency key.
    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            Self::Init
                | Self::Import
                | Self::Build
                | Self::Deploy
                | Self::Rollback
                | Self::Capability
                | Self::Preview
                | Self::Agent
        )
    }

    /// Environment is mandatory for commands that target one deployment
    /// environment.
    pub fn requires_environment(self) -> bool {
        matches!(
            self,
            Self::Deploy | Self::Rollback | Self::Health | Self::Preview
        )
    }

    /// Application is mandatory for everything except registration commands.
    pub fn requires_application(self) -> bool {
        !matches!(self, Self::Init | Self::Import)
    }

    /// All stable commands, for help output and conformance tests.
    pub fn all() -> &'static [CliCommand] {
        &[
            Self::Init,
            Self::Import,
            Self::Inspect,
            Self::Dev,
            Self::Agent,
            Self::Preview,
            Self::Capability,
            Self::Resources,
            Self::Build,
            Self::Deploy,
            Self::Deployments,
            Self::Logs,
            Self::Health,
            Self::Rollback,
            Self::Doctor,
        ]
    }
}

impl std::fmt::Display for CliCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.canonical_name())
    }
}

/// Who is invoking the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum CliActor {
    /// A human operator.
    Human { name: String },
    /// An agent session acting through the CLI.
    Agent { session_id: String, name: String },
    /// A platform controller or automation.
    Platform { name: String },
}

impl CliActor {
    pub fn human(name: impl Into<String>) -> Self {
        Self::Human { name: name.into() }
    }

    pub fn agent(session_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Agent {
            session_id: session_id.into(),
            name: name.into(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Human { .. } => "human",
            Self::Agent { .. } => "agent",
            Self::Platform { .. } => "platform",
        }
    }

    /// Agent and platform actors may not perform human-only operations.
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Human { .. })
    }
}

/// One CLI invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliRequest {
    pub command: CliCommand,
    pub actor: CliActor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<ApplicationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default)]
    pub arguments: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ExplicitApproval>,
}

impl CliRequest {
    pub fn new(command: CliCommand, actor: CliActor) -> Self {
        Self {
            command,
            actor,
            application_id: None,
            environment: None,
            arguments: BTreeMap::new(),
            idempotency_key: None,
            approval: None,
        }
    }

    pub fn on_application(mut self, application_id: ApplicationId) -> Self {
        self.application_id = Some(application_id);
        self
    }

    pub fn in_environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    pub fn with_argument(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.arguments.insert(key.into(), value.into());
        self
    }

    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn with_approval(mut self, approval: ExplicitApproval) -> Self {
        self.approval = Some(approval);
        self
    }
}

/// Machine-readable CLI envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliResponse {
    pub command: String,
    pub ok: bool,
    /// True when the response was replayed from an idempotency key.
    pub replayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_id: Option<ApplicationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub at: DateTime<Utc>,
}

impl CliResponse {
    /// True when any finding is an error.
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// Renders the envelope as machine-readable JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    /// Human-facing one-line summary for terminal output.
    pub fn to_event_detail(&self) -> String {
        format!(
            "{} {} ({} findings){}",
            self.command,
            if self.ok { "ok" } else { "FAILED" },
            self.findings.len(),
            if self.replayed { " replayed" } else { "" }
        )
    }
}

/// Platform state the CLI reads and mutates.
#[derive(Debug, Default)]
pub struct PlatformState {
    pub applications: Vec<ApplicationId>,
    pub environments: BTreeMap<ApplicationId, Vec<String>>,
    pub capabilities: BTreeMap<ApplicationId, Vec<String>>,
    pub resources: BTreeMap<ApplicationId, Vec<String>>,
    pub deployments: BTreeMap<ApplicationId, Vec<String>>,
    pub health: BTreeMap<ApplicationId, HealthSnapshot>,
    pub logs: LogStore,
    pub events: EventLog,
    pub audit: AuditLog,
    pub plugins: PluginRegistry,
    pub evidence: Option<EvidenceStore>,
}

impl PlatformState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_application(&mut self, application_id: ApplicationId, environments: Vec<String>) {
        if !self.applications.contains(&application_id) {
            self.applications.push(application_id);
        }
        self.environments.insert(application_id, environments);
    }

    pub fn knows(&self, application_id: ApplicationId) -> bool {
        self.applications.contains(&application_id)
    }
}

/// Executes CLI requests against platform state.
///
/// Every mutating command must carry an idempotency key; a repeated key
/// replays the recorded response and performs no second side effect. Unknown
/// applications, missing environments, and permission denials are returned as
/// error findings with recovery guidance rather than panics.
#[derive(Debug, Default)]
pub struct Cli {
    executed: BTreeMap<String, CliResponse>,
}

impl Cli {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs one request. `state` is mutated for side-effecting commands.
    pub fn execute(
        &mut self,
        state: &mut PlatformState,
        request: CliRequest,
        secrets: &[&str],
        now: DateTime<Utc>,
    ) -> Result<CliResponse> {
        let command = request.command;

        // Permission boundary before anything else.
        if let Some(denied) = Self::authorize(&request) {
            return Ok(self.failure(&request, now, vec![denied]));
        }

        // Idempotency: a mutating command without a key is rejected outright.
        if command.is_mutating() && request.idempotency_key.is_none() {
            return Ok(self.failure(
                &request,
                now,
                vec![Finding::error(
                    "cli.idempotency_required",
                    format!(
                        "'{}' changes platform state and needs an idempotency key",
                        command
                    ),
                )
                .with_recovery("re-run with --idempotency-key <key> so a retry is safe")],
            ));
        }
        if let Some(key) = &request.idempotency_key {
            if let Some(replay) = self.executed.get(key) {
                let mut replayed = replay.clone();
                replayed.replayed = true;
                return Ok(replayed);
            }
        }

        // Structural validation of required targets.
        let mut findings = Vec::new();
        if command.requires_application() {
            match request.application_id {
                None => findings.push(
                    Finding::error(
                        "cli.application_required",
                        format!("'{}' needs an application", command),
                    )
                    .with_recovery("pass --application <id> or run `labrys import .` first"),
                ),
                Some(id) if !state.knows(id) => findings.push(
                    Finding::error(
                        "cli.application_unknown",
                        format!("application {id} is not registered"),
                    )
                    .with_recovery("run `labrys import .` to register this project"),
                ),
                Some(_) => {}
            }
        }
        if command.requires_environment() {
            let wanted = request.environment.as_deref();
            let known = request
                .application_id
                .and_then(|id| state.environments.get(&id))
                .map(|envs| envs.iter().any(|e| Some(e.as_str()) == wanted))
                .unwrap_or(false);
            if wanted.is_none() || !known {
                findings.push(
                    Finding::error(
                        "cli.environment_unknown",
                        format!("'{}' needs a known environment, got {:?}", command, wanted),
                    )
                    .with_recovery("list environments with `labrys inspect`"),
                );
            }
        }
        if !findings.is_empty() {
            return Ok(self.failure(&request, now, findings));
        }

        let (ok, data, command_findings) = match command {
            CliCommand::Init | CliCommand::Import => {
                let id = request.application_id.unwrap_or_default();
                let path = request
                    .arguments
                    .get("path")
                    .cloned()
                    .unwrap_or_else(|| ".".to_string());
                state.add_application(
                    id,
                    vec!["development".to_string(), "production".to_string()],
                );
                state.audit.append(
                    now,
                    request.actor.kind_label(),
                    command.canonical_name(),
                    id.to_string(),
                    &format!("registered from {path}"),
                    secrets,
                );
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "path": path,
                        "environments": ["development", "production"],
                    })),
                    vec![Finding::info(
                        "cli.registered",
                        format!("application {id} registered from {path}"),
                    )],
                )
            }
            CliCommand::Inspect => {
                let id = request.application_id.expect("validated above");
                let capabilities = state.capabilities.get(&id).cloned().unwrap_or_default();
                let resources = state.resources.get(&id).cloned().unwrap_or_default();
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "environments": state.environments.get(&id).cloned().unwrap_or_default(),
                        "capabilities": capabilities,
                        "resources": resources,
                        "deployments": state.deployments.get(&id).cloned().unwrap_or_default(),
                    })),
                    vec![Finding::info(
                        "cli.inspected",
                        format!(
                            "{} capabilities, {} resources",
                            capabilities.len(),
                            resources.len()
                        ),
                    )],
                )
            }
            CliCommand::Doctor => {
                let id = request.application_id.expect("validated above");
                let mut doctor = Vec::new();
                match state.health.get(&id) {
                    None => doctor.push(
                        Finding::warning(
                            "doctor.no_health_evidence",
                            "no platform health observation recorded",
                        )
                        .with_recovery("run a health probe or wait for the controller"),
                    ),
                    Some(snapshot) if snapshot.is_stale(now) => doctor.push(
                        Finding::warning(
                            "doctor.stale_health",
                            format!("health evidence for '{}' is stale", snapshot.target),
                        )
                        .with_recovery("re-probe before trusting this result"),
                    ),
                    Some(snapshot) => doctor.push(Finding::info(
                        "doctor.health",
                        format!("{} is {:?}", snapshot.target, snapshot.effective_state(now)),
                    )),
                }
                for report in state.plugins.report() {
                    match report.state {
                        crate::plugin::PluginState::Active => doctor.push(Finding::info(
                            "doctor.plugin",
                            format!(
                                "{} ({}) active",
                                report.name,
                                report.family.canonical_name()
                            ),
                        )),
                        other => doctor.push(
                            Finding::error(
                                "doctor.plugin_unavailable",
                                format!(
                                    "{} ({}) is {other:?}",
                                    report.name,
                                    report.family.canonical_name()
                                ),
                            )
                            .with_recovery(report.detail.clone()),
                        ),
                    }
                }
                if let Some(store) = &state.evidence {
                    for record in store.evidence().iter().filter(|e| !e.passed) {
                        doctor.push(
                            Finding::error("doctor.failed_evidence", record.to_event_detail())
                                .with_recovery(record.recovery.clone().unwrap_or_else(|| {
                                    "inspect the failing check and re-run verification".to_string()
                                })),
                        );
                    }
                }
                let ok = !doctor.iter().any(|f| f.severity == Severity::Error);
                (
                    ok,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "checks": doctor.iter().map(|f| f.to_json_value()).collect::<Vec<_>>(),
                    })),
                    doctor,
                )
            }
            CliCommand::Logs => {
                let id = request.application_id.expect("validated above");
                let source = request.arguments.get("source").and_then(|name| {
                    [
                        LogSource::Agent,
                        LogSource::Build,
                        LogSource::Runtime,
                        LogSource::Deployment,
                        LogSource::Capability,
                        LogSource::Resource,
                    ]
                    .into_iter()
                    .find(|s| s.canonical_name() == name.as_str())
                });
                let entries: Vec<&crate::observability::LogEntry> = match source {
                    Some(source) => state.logs.for_source(source),
                    None => state.logs.entries().iter().collect(),
                };
                let rendered: Vec<Value> = entries
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "sequence": entry.sequence,
                            "source": entry.source.canonical_name(),
                            "level": format!("{:?}", entry.level),
                            "message": entry.message,
                            "trace": entry.correlation.trace_id,
                        })
                    })
                    .collect();
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "entries": rendered,
                    })),
                    vec![Finding::info(
                        "cli.logs",
                        format!("{} entries", rendered.len()),
                    )],
                )
            }
            CliCommand::Health => {
                let id = request.application_id.expect("validated above");
                match state.health.get(&id) {
                    None => (
                        false,
                        None,
                        vec![Finding::error(
                            "cli.no_health",
                            "no platform health evidence for this application",
                        )
                        .with_recovery("run `labrys doctor` to see which probe is missing")],
                    ),
                    Some(snapshot) => {
                        let effective = snapshot.effective_state(now);
                        let stale = snapshot.is_stale(now);
                        let mut command_findings = vec![Finding {
                            code: "cli.health".to_string(),
                            severity: if stale || effective != crate::HealthState::Healthy {
                                Severity::Warning
                            } else {
                                Severity::Info
                            },
                            message: format!("{} is {effective:?}", snapshot.target),
                            recovery: stale.then(|| {
                                "re-probe; this observation is older than its ttl".to_string()
                            }),
                            resource: None,
                        }];
                        if let Some(detail) = &snapshot.detail {
                            command_findings
                                .push(Finding::info("cli.health_detail", detail.clone()));
                        }
                        let ok = effective == crate::HealthState::Healthy && !stale;
                        (
                            ok,
                            Some(serde_json::json!({
                                "state": format!("{effective:?}"),
                                "stale": stale,
                                "observed_at": snapshot.observed_at.to_rfc3339(),
                            })),
                            command_findings,
                        )
                    }
                }
            }
            CliCommand::Rollback => {
                let id = request.application_id.expect("validated above");
                let granted = request
                    .approval
                    .as_ref()
                    .map(|a| a.approved && !a.approver.trim().is_empty())
                    .unwrap_or(false);
                if !granted {
                    return Ok(self.failure(
                        &request,
                        now,
                        vec![Finding::error(
                            "cli.approval_required",
                            "rollback requires a granted, named human approval",
                        )
                        .with_recovery("re-run with --approve-by <name> --target <id>")],
                    ));
                }
                let approval = request.approval.as_ref().expect("granted above");
                let target = request
                    .arguments
                    .get("target")
                    .cloned()
                    .unwrap_or_else(|| "previous".to_string());
                state.audit.append(
                    now,
                    &approval.approver,
                    "rollback",
                    id.to_string(),
                    "traffic moved; database data not restored",
                    secrets,
                );
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "target": target,
                        "approved_by": approval.approver,
                        "database_data_restored": false,
                        "warning": crate::deployment::DATABASE_DATA_WARNING,
                    })),
                    vec![Finding::info(
                        "cli.rolled_back",
                        format!("rolled back to {target}"),
                    )],
                )
            }
            CliCommand::Deploy | CliCommand::Build => {
                let id = request.application_id.expect("validated above");
                let environment = request.environment.clone().expect("validated above");
                if environment == "production" && !request.actor.is_human() {
                    return Ok(self.failure(
                        &request,
                        now,
                        vec![Finding::error(
                            "cli.permission_denied",
                            "production deployment requires a human actor",
                        )
                        .with_recovery(
                            "a human must run `labrys deploy --environment production`",
                        )],
                    ));
                }
                let digest = request
                    .arguments
                    .get("artifact")
                    .cloned()
                    .unwrap_or_else(|| "sha256:unspecified".to_string());
                state
                    .deployments
                    .entry(id)
                    .or_default()
                    .push(format!("{environment}:{digest}"));
                state.audit.append(
                    now,
                    request.actor.kind_label(),
                    command.canonical_name(),
                    id.to_string(),
                    &format!("{environment} -> {digest}"),
                    secrets,
                );
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        "environment": environment,
                        "artifact": digest,
                        "health_gate": true,
                    })),
                    vec![Finding::info(
                        "cli.converging",
                        format!("{command} {digest} requested; promotion waits for health"),
                    )],
                )
            }
            CliCommand::Capability
            | CliCommand::Resources
            | CliCommand::Deployments
            | CliCommand::Preview
            | CliCommand::Dev
            | CliCommand::Agent => {
                let id = request.application_id.expect("validated above");
                let (key, values) = match command {
                    CliCommand::Capability => {
                        ("capabilities", state.capabilities.get(&id).cloned())
                    }
                    CliCommand::Resources => ("resources", state.resources.get(&id).cloned()),
                    CliCommand::Deployments => ("deployments", state.deployments.get(&id).cloned()),
                    _ => (command.canonical_name(), None),
                };
                let values = values.unwrap_or_default();
                if command.is_mutating() {
                    // Capability/preview/agent mutations record intent.
                    state.audit.append(
                        now,
                        request.actor.kind_label(),
                        command.canonical_name(),
                        id.to_string(),
                        "requested",
                        secrets,
                    );
                }
                (
                    true,
                    Some(serde_json::json!({
                        "application_id": id.to_string(),
                        key: values,
                    })),
                    vec![Finding::info(
                        format!("cli.{}", command.canonical_name()),
                        format!("{} entries", values.len()),
                    )],
                )
            }
        };

        // Registration commands create the application, so the response is
        // tied to the id they minted rather than the request's.
        let resolved_application = if ok && matches!(command, CliCommand::Init | CliCommand::Import)
        {
            data.as_ref()
                .and_then(|d| d.get("application_id"))
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
        } else {
            None
        };
        let response = CliResponse {
            command: command.canonical_name().to_string(),
            ok,
            replayed: false,
            application_id: resolved_application.or(request.application_id),
            environment: request.environment.clone(),
            findings: command_findings,
            data,
            idempotency_key: request.idempotency_key.clone(),
            at: now,
        };
        if let Some(key) = request.idempotency_key {
            self.executed.insert(key, response.clone());
        }
        Ok(response)
    }

    /// Permission boundary: which actors may run which commands. Agent
    /// sessions cannot drive lifecycle mutations, and human-only operations
    /// (rollback) are refused for every non-human actor.
    fn authorize(request: &CliRequest) -> Option<Finding> {
        if matches!(request.actor, CliActor::Agent { .. })
            && matches!(
                request.command,
                CliCommand::Init
                    | CliCommand::Import
                    | CliCommand::Deploy
                    | CliCommand::Rollback
                    | CliCommand::Build
            )
        {
            return Some(
                Finding::error(
                    "cli.permission_denied",
                    format!(
                        "an agent session may not run '{}'",
                        request.command.canonical_name()
                    ),
                )
                .with_recovery("a human operator must run this command"),
            );
        }
        if request.command == CliCommand::Rollback && !request.actor.is_human() {
            return Some(
                Finding::error(
                    "cli.permission_denied",
                    "rollback is a human-only operation",
                )
                .with_recovery("a named human must issue the rollback"),
            );
        }
        if matches!(request.actor, CliActor::Platform { .. })
            && matches!(request.command, CliCommand::Deploy | CliCommand::Rollback)
        {
            return Some(
                Finding::error(
                    "cli.permission_denied",
                    format!(
                        "platform automation may not issue '{}'",
                        request.command.canonical_name()
                    ),
                )
                .with_recovery("route this through a human approval flow"),
            );
        }
        None
    }

    fn failure(
        &self,
        request: &CliRequest,
        now: DateTime<Utc>,
        findings: Vec<Finding>,
    ) -> CliResponse {
        CliResponse {
            command: request.command.canonical_name().to_string(),
            ok: false,
            replayed: false,
            application_id: request.application_id,
            environment: request.environment.clone(),
            findings,
            data: None,
            idempotency_key: request.idempotency_key.clone(),
            at: now,
        }
    }
}

// ---------------------------------------------------------------------------
// Dashboard read/write boundaries
// ---------------------------------------------------------------------------

/// Dashboard surface areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DashboardSection {
    Apps,
    Overview,
    Vibe,
    CodeChanges,
    Preview,
    Capabilities,
    Resources,
    Runtime,
    Deployments,
    Logs,
    Secrets,
    Settings,
}

impl DashboardSection {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Overview => "overview",
            Self::Vibe => "vibe",
            Self::CodeChanges => "code_changes",
            Self::Preview => "preview",
            Self::Capabilities => "capabilities",
            Self::Resources => "resources",
            Self::Runtime => "runtime",
            Self::Deployments => "deployments",
            Self::Logs => "logs",
            Self::Secrets => "secrets",
            Self::Settings => "settings",
        }
    }

    /// Secrets are reference-only and always read from the dashboard.
    pub fn is_read_only(self) -> bool {
        matches!(self, Self::Secrets | Self::Logs | Self::CodeChanges)
    }

    /// Writes that change platform state require a granted, named approval.
    pub fn requires_approval(self) -> bool {
        matches!(
            self,
            Self::Deployments | Self::Capabilities | Self::Resources | Self::Settings
        )
    }

    pub fn all() -> &'static [DashboardSection] {
        &[
            Self::Apps,
            Self::Overview,
            Self::Vibe,
            Self::CodeChanges,
            Self::Preview,
            Self::Capabilities,
            Self::Resources,
            Self::Runtime,
            Self::Deployments,
            Self::Logs,
            Self::Secrets,
            Self::Settings,
        ]
    }
}

impl std::fmt::Display for DashboardSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.canonical_name())
    }
}

/// Who a displayed entry is attributed to. Agent and platform output are
/// never merged into one claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Attribution {
    Agent { session_id: String, name: String },
    Platform { name: String },
    Human { name: String },
}

impl Attribution {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Agent { .. } => "agent",
            Self::Platform { .. } => "platform",
            Self::Human { .. } => "human",
        }
    }
}

/// One row in a dashboard section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardEntry {
    pub key: String,
    pub value: String,
    pub attribution: Attribution,
    /// Platform-observed state; `None` for agent-attributed rows, which never
    /// carry health.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::HealthState>,
    /// Where the evidence came from, e.g. `platform:health-probe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_source: Option<String>,
}

/// A secret row: reference and version only, never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretRow {
    pub ref_id: String,
    pub version_hint: String,
    pub environment: String,
}

/// Rendered dashboard state for one application environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardView {
    pub application_id: ApplicationId,
    pub environment: String,
    /// Computed from platform-owned evidence only.
    pub production_healthy: bool,
    pub sections: BTreeMap<DashboardSection, Vec<DashboardEntry>>,
    pub secrets: Vec<SecretRow>,
    /// The agent's own completion claim, kept separate from platform state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_completion_claim: Option<String>,
    pub at: DateTime<Utc>,
}

impl DashboardView {
    pub fn entries(&self, section: DashboardSection) -> &[DashboardEntry] {
        self.sections
            .get(&section)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// True only when platform evidence says healthy. An agent claim can
    /// never flip this.
    pub fn is_production_healthy(&self) -> bool {
        self.production_healthy
    }

    /// Platform-attributed rows carrying a failure state.
    pub fn platform_failures(&self) -> Vec<&DashboardEntry> {
        self.sections
            .values()
            .flatten()
            .filter(|entry| {
                entry.attribution.kind_label() == "platform"
                    && matches!(
                        entry.state,
                        Some(crate::HealthState::Failed) | Some(crate::HealthState::Degraded)
                    )
            })
            .collect()
    }
}

/// Renders the dashboard for one application environment.
///
/// Agent events become agent-attributed rows; platform health, deployment,
/// resource, and capability evidence become platform-attributed rows with an
/// evidence source. Production health is derived from the platform snapshot
/// only, so an agent reporting completion while deployment health fails shows
/// the failure and never reports the application as healthy.
pub fn render_dashboard(
    state: &PlatformState,
    application_id: ApplicationId,
    environment: &str,
    now: DateTime<Utc>,
) -> Result<DashboardView> {
    if !state.knows(application_id) {
        return Err(CoreError::NotFound(application_id.to_string()));
    }
    let mut sections: BTreeMap<DashboardSection, Vec<DashboardEntry>> = DashboardSection::all()
        .iter()
        .map(|section| (*section, Vec::new()))
        .collect();

    // Health is platform evidence, never an agent statement.
    let snapshot = state.health.get(&application_id);
    let effective = snapshot.map(|s| s.effective_state(now));
    let production_healthy = snapshot
        .map(|s| !s.is_stale(now) && s.effective_state(now) == crate::HealthState::Healthy)
        .unwrap_or(false);
    sections
        .get_mut(&DashboardSection::Overview)
        .expect("section exists")
        .push(DashboardEntry {
            key: "health".to_string(),
            value: snapshot
                .map(|s| s.target.clone())
                .unwrap_or_else(|| "no probe".to_string()),
            attribution: Attribution::Platform {
                name: "health-probe".to_string(),
            },
            state: effective,
            evidence_source: snapshot.map(|_| "platform:health-probe".to_string()),
        });

    // Agent stream: normalized claims, attributed to the session.
    let mut claim: Option<String> = None;
    for event in state.events.for_application(application_id) {
        if let crate::observability::EventActor::Agent { session_id, name } = &event.actor {
            if event.action == crate::observability::EventAction::VerificationFinished {
                claim = Some(event.to_event_detail());
            }
            sections
                .get_mut(&DashboardSection::Vibe)
                .expect("section exists")
                .push(DashboardEntry {
                    key: format!("e{}", event.sequence),
                    value: event.to_event_detail(),
                    attribution: Attribution::Agent {
                        session_id: session_id.to_string(),
                        name: name.clone(),
                    },
                    state: None,
                    evidence_source: Some("agent:session-stream".to_string()),
                });
        } else {
            sections
                .get_mut(&DashboardSection::Overview)
                .expect("section exists")
                .push(DashboardEntry {
                    key: format!("e{}", event.sequence),
                    value: event.to_event_detail(),
                    attribution: Attribution::Platform {
                        name: event.actor.kind_label().to_string(),
                    },
                    state: None,
                    evidence_source: Some("platform:event-log".to_string()),
                });
        }
    }

    // Deployment failures from platform evidence stay visible even when the
    // agent stream claims completion.
    for entry in state.logs.for_source(LogSource::Deployment) {
        if entry.level == crate::deployment::LogLevel::Error {
            sections
                .get_mut(&DashboardSection::Deployments)
                .expect("section exists")
                .push(DashboardEntry {
                    key: format!("log{}", entry.sequence),
                    value: entry.message.clone(),
                    attribution: Attribution::Platform {
                        name: "deployment-controller".to_string(),
                    },
                    state: Some(crate::HealthState::Failed),
                    evidence_source: Some("platform:deployment-log".to_string()),
                });
        }
    }

    for capability in state
        .capabilities
        .get(&application_id)
        .cloned()
        .unwrap_or_default()
    {
        sections
            .get_mut(&DashboardSection::Capabilities)
            .expect("section exists")
            .push(DashboardEntry {
                key: capability.clone(),
                value: capability,
                attribution: Attribution::Platform {
                    name: "capability-controller".to_string(),
                },
                state: None,
                evidence_source: Some("platform:binding".to_string()),
            });
    }
    for resource in state
        .resources
        .get(&application_id)
        .cloned()
        .unwrap_or_default()
    {
        sections
            .get_mut(&DashboardSection::Resources)
            .expect("section exists")
            .push(DashboardEntry {
                key: resource.clone(),
                value: resource,
                attribution: Attribution::Platform {
                    name: "resource-controller".to_string(),
                },
                state: None,
                evidence_source: Some("platform:resource".to_string()),
            });
    }
    for deployment in state
        .deployments
        .get(&application_id)
        .cloned()
        .unwrap_or_default()
    {
        sections
            .get_mut(&DashboardSection::Deployments)
            .expect("section exists")
            .push(DashboardEntry {
                key: deployment.clone(),
                value: deployment,
                attribution: Attribution::Platform {
                    name: "deployment-controller".to_string(),
                },
                state: None,
                evidence_source: Some("platform:deployment".to_string()),
            });
    }
    for report in state.plugins.report() {
        if report.state != crate::plugin::PluginState::Active {
            sections
                .get_mut(&DashboardSection::Settings)
                .expect("section exists")
                .push(DashboardEntry {
                    key: report.id,
                    value: report.detail,
                    attribution: Attribution::Platform {
                        name: "plugin-registry".to_string(),
                    },
                    state: Some(crate::HealthState::Failed),
                    evidence_source: Some("platform:plugin-handshake".to_string()),
                });
        }
    }
    for entry in state.logs.entries() {
        sections
            .get_mut(&DashboardSection::Logs)
            .expect("section exists")
            .push(DashboardEntry {
                key: format!("{}", entry.sequence),
                value: format!("[{}] {}", entry.source.canonical_name(), entry.message),
                attribution: Attribution::Platform {
                    name: entry.source.canonical_name().to_string(),
                },
                state: None,
                evidence_source: Some(format!("platform:log-{}", entry.source.canonical_name())),
            });
    }

    Ok(DashboardView {
        application_id,
        environment: environment.to_string(),
        production_healthy,
        sections,
        secrets: Vec::new(),
        agent_completion_claim: claim,
        at: now,
    })
}

/// Attaches secret references (never values) to the Secrets section.
pub fn attach_secret_rows(view: &mut DashboardView, rows: Vec<SecretRow>) {
    view.secrets = rows;
}

/// Authorizes a dashboard write against the section's boundary.
pub fn authorize_dashboard_write(
    section: DashboardSection,
    actor: &CliActor,
    approval: Option<&ExplicitApproval>,
) -> Result<()> {
    if section.is_read_only() {
        return Err(CoreError::PermissionDenied(format!(
            "the '{}' section is read-only",
            section.canonical_name()
        )));
    }
    if matches!(actor, CliActor::Agent { .. }) {
        return Err(CoreError::PermissionDenied(
            "agent sessions cannot write through the dashboard; use the platform API".to_string(),
        ));
    }
    if section.requires_approval() {
        let granted = approval
            .map(|a| a.approved && !a.approver.trim().is_empty())
            .unwrap_or(false);
        if !granted {
            return Err(CoreError::ApprovalRequired(format!(
                "writing to '{}' requires a granted, named human approval",
                section.canonical_name()
            )));
        }
    }
    Ok(())
}
