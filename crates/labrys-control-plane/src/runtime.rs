//! Bounded Docker container execution built on the pure core contracts.
//!
//! `labrys-core` plans builds, validates [`SandboxLimits`], and evaluates
//! health semantics without I/O. This module performs the I/O: it turns a
//! [`RuntimeConfig`] into structured `docker` CLI invocations with explicit
//! resource limits, cancellation, platform-owned health probes, redacted
//! diagnostics, and cleanup. There is no shell anywhere on this path — every
//! argument is passed as a separate `argv` entry, so a crafted image name,
//! container name, or workspace path can never escape into command execution.
//!
//! Boundaries:
//! - Limits are enforced *before* anything is scheduled
//!   ([`enforce_limits_before_schedule`]); the effective limits travel with
//!   every outcome for events and evidence.
//! - A development process is never promoted into a production image
//!   ([`verify_production_artifact`]); production always mints an OCI-bound
//!   artifact, development never does.
//! - Readiness comes only from a platform-owned probe ([`probe_health`]).
//!   Agent completion claims are not a variant on this path and can never
//!   establish health.
//! - When Docker is absent the executor reports an environment blocker
//!   ([`RuntimeAvailability::Unavailable`]); it never simulates a pass.
//! - All free-text diagnostics are redacted against caller-supplied secrets
//!   before they are returned or persisted.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use labrys_core::{BuildResult, Correlation, HealthStatus, ResourcePhase, RuntimeConfig};

use crate::error::{ControlPlaneError, Result};
use crate::redact;

/// Identity carried by every execution: which application, environment,
/// workspace, and session the container belongs to, plus the trace that ties
/// its events, logs, and evidence together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionIdentity {
    pub application_id: String,
    pub environment_id: Option<String>,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    pub trace_id: String,
}

impl ExecutionIdentity {
    pub fn new(application_id: impl Into<String>, trace_id: impl Into<String>) -> Result<Self> {
        let application_id = application_id.into();
        let trace_id = trace_id.into();
        if application_id.trim().is_empty() {
            return Err(ControlPlaneError::Execution(
                "execution requires an application id".to_string(),
            ));
        }
        if trace_id.trim().is_empty() {
            return Err(ControlPlaneError::Execution(
                "execution requires a trace id".to_string(),
            ));
        }
        Ok(Self {
            application_id,
            environment_id: None,
            workspace_id: None,
            session_id: None,
            trace_id,
        })
    }

    pub fn with_environment(mut self, environment_id: impl Into<String>) -> Self {
        self.environment_id = Some(environment_id.into());
        self
    }

    pub fn with_workspace(mut self, workspace_id: impl Into<String>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Correlation stamped on every event and log this execution emits.
    pub fn correlation(&self) -> Correlation {
        Correlation {
            trace_id: self.trace_id.clone(),
            session_id: self.session_id.as_ref().and_then(|s| s.parse().ok()),
            application_id: self.application_id.parse().ok(),
            environment_id: self.environment_id.as_ref().and_then(|s| s.parse().ok()),
        }
    }
}

/// Whether the container runtime can be used right now.
///
/// `Unavailable` is an environment blocker for callers: surface it, do not
/// fall back to [`labrys_core::simulate_build`] as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeAvailability {
    Available { docker_version: String },
    Unavailable { reason: String },
}

impl RuntimeAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn require(&self) -> Result<&str> {
        match self {
            Self::Available { docker_version } => Ok(docker_version),
            Self::Unavailable { reason } => Err(ControlPlaneError::Unavailable(reason.clone())),
        }
    }
}

/// The limits actually applied to one execution, for events and evidence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveLimits {
    pub cpu_millicpus: u32,
    pub memory_mb: u32,
    pub timeout_secs: u64,
    pub readonly_root: bool,
    pub network: String,
    pub drop_capabilities: Vec<String>,
    pub max_processes: u32,
    pub port_exposed: bool,
}

impl EffectiveLimits {
    pub fn from_config(config: &RuntimeConfig) -> Self {
        Self {
            cpu_millicpus: config.limits.cpu_millicpus,
            memory_mb: config.limits.memory_mb,
            timeout_secs: config.limits.timeout_secs,
            readonly_root: config.limits.readonly_root,
            network: format!("{:?}", config.limits.network).to_lowercase(),
            drop_capabilities: config.limits.drop_capabilities.clone(),
            max_processes: config.limits.max_processes,
            port_exposed: config.limits.allows_port_exposure(),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "cpu={}m memory={}MiB timeout={}s readonly_root={} network={} pids={} exposed={}",
            self.cpu_millicpus,
            self.memory_mb,
            self.timeout_secs,
            self.readonly_root,
            self.network,
            self.max_processes,
            self.port_exposed
        )
    }
}

/// Rejects unschedulable work before any container is created.
///
/// Validates the sandbox limits and refuses a disabled-network run that still
/// asks for port exposure semantics the sandbox cannot provide. Callers must
/// run this before `docker build`/`docker run` and record
/// [`EffectiveLimits::from_config`] with the outcome.
pub fn enforce_limits_before_schedule(config: &RuntimeConfig) -> Result<()> {
    config.limits.validate().map_err(ControlPlaneError::Core)?;
    if config.port == 0 {
        return Err(ControlPlaneError::Execution(
            "execution requires a container port in 1..=65535".to_string(),
        ));
    }
    Ok(())
}

/// Production interchange boundary: production always carries an OCI image,
/// development never is one.
///
/// Returns the OCI reference for production configs and fails for development
/// configs, so a dev-server process can never be promoted into a production
/// image implicitly.
pub fn verify_production_artifact(config: &RuntimeConfig) -> Result<String> {
    match config.profile {
        labrys_core::RuntimeProfile::Production => config.oci_image.clone().ok_or_else(|| {
            ControlPlaneError::Execution(
                "production execution requires an OCI image reference".to_string(),
            )
        }),
        labrys_core::RuntimeProfile::Development => Err(ControlPlaneError::Execution(
            "development process must never be promoted as a production image".to_string(),
        )),
    }
}

/// Maps platform-observed health to the resource phase controllers converge
/// on. `Healthy` is the only ready state; `Starting` stays provisioning;
/// anything else is degraded. Agent claims never reach this function.
pub fn prospective_phase(health: &HealthStatus) -> ResourcePhase {
    match health {
        HealthStatus::Healthy => ResourcePhase::Ready,
        HealthStatus::Starting => ResourcePhase::Provisioning,
        HealthStatus::Unhealthy { .. } | HealthStatus::Unknown { .. } => ResourcePhase::Degraded,
    }
}

/// Structured `docker build` arguments: `build -f <dockerfile> -t <tag> <context>`.
pub fn docker_build_args(dockerfile: &Path, context: &Path, tag: &str) -> Result<Vec<String>> {
    validate_tag(tag)?;
    Ok(vec![
        "build".to_string(),
        "-f".to_string(),
        path_arg(dockerfile)?,
        "-t".to_string(),
        tag.to_string(),
        path_arg(context)?,
    ])
}

/// Structured `docker run` arguments for one bounded, isolated container.
///
/// Applies CPU (`--cpus`), memory (`--memory`), PID (`--pids-limit`),
/// read-only root, dropped capabilities, and network posture. A disabled
/// network publishes no ports and runs with `--network=none`; otherwise the
/// container port is published to a loopback-bound host port.
pub fn docker_run_args(
    config: &RuntimeConfig,
    container_name: &str,
    image: &str,
) -> Result<Vec<String>> {
    enforce_limits_before_schedule(config)?;
    validate_container_name(container_name)?;
    validate_tag(image)?;
    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container_name.to_string(),
    ];
    let cpus = (f64::from(config.limits.cpu_millicpus) / 1000.0).max(0.01);
    args.push("--cpus".to_string());
    args.push(format!("{cpus:.2}"));
    args.push("--memory".to_string());
    args.push(format!("{}m", config.limits.memory_mb));
    args.push("--pids-limit".to_string());
    args.push(config.limits.max_processes.to_string());
    if config.limits.readonly_root {
        args.push("--read-only".to_string());
    }
    for cap in &config.limits.drop_capabilities {
        let cap = cap.trim();
        if !cap.is_empty() {
            args.push("--cap-drop".to_string());
            args.push(cap.to_string());
        }
    }
    match config.limits.network {
        labrys_core::NetworkMode::Disabled => {
            args.push("--network".to_string());
            args.push("none".to_string());
        }
        labrys_core::NetworkMode::Isolated | labrys_core::NetworkMode::Bridged => {
            args.push("--network".to_string());
            args.push("bridge".to_string());
            args.push("-p".to_string());
            args.push(format!("127.0.0.1::{}", config.port));
        }
    }
    args.push(image.to_string());
    Ok(args)
}

/// A container started by the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningContainer {
    pub id: String,
    pub name: String,
    pub host_port: Option<u16>,
}

impl RunningContainer {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self> {
        let id = id.into();
        let name = name.into();
        if id.trim().is_empty() {
            return Err(ControlPlaneError::Execution(
                "running container requires a container id".to_string(),
            ));
        }
        Ok(Self {
            id,
            name,
            host_port: None,
        })
    }
}

/// Bounded container execution contract.
///
/// Implementations perform real process I/O; the trait keeps callers
/// testable with scripted or unavailable doubles. Health comes only from
/// platform-owned probes — there is no agent-claim input on this trait.
#[async_trait]
pub trait ContainerExecutor: Send + Sync {
    async fn check_available(&self) -> RuntimeAvailability;
    async fn build(
        &self,
        dockerfile: &Path,
        context: &Path,
        tag: &str,
        limits: &labrys_core::SandboxLimits,
        identity: &ExecutionIdentity,
    ) -> BuildResult;
    async fn run(
        &self,
        config: &RuntimeConfig,
        image: &str,
        container_name: &str,
        identity: &ExecutionIdentity,
    ) -> Result<RunningContainer>;
    async fn stop(&self, container_id: &str) -> Result<()>;
    async fn remove(&self, container_id: &str) -> Result<()>;
    /// Stops and removes, tolerating already-gone containers so expiry,
    /// failure, and revocation paths are idempotent.
    async fn cleanup(&self, container_id: &str) -> Result<()> {
        let _ = self.stop(container_id).await;
        self.remove(container_id).await
    }
    async fn logs(&self, container_id: &str, tail: usize) -> Result<String>;
    /// Platform-owned health probe for a started workload.
    async fn health(&self, config: &RuntimeConfig, host: &str, port: u16) -> HealthStatus;
}

/// Docker CLI executor: the production [`ContainerExecutor`].
///
/// Invokes the `docker` binary with structured arguments, enforces the
/// sandbox timeout via cancellation, and redacts known secret values out of
/// every diagnostic it returns.
#[derive(Debug, Clone)]
pub struct DockerExecutor {
    docker_bin: PathBuf,
    secrets: Vec<String>,
}

impl DockerExecutor {
    pub fn new(docker_bin: PathBuf) -> Self {
        Self {
            docker_bin,
            secrets: Vec::new(),
        }
    }

    pub fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets;
        self
    }

    pub fn from_path() -> Self {
        Self::new(PathBuf::from("docker"))
    }

    fn redact_text(&self, text: &str) -> String {
        let refs: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        redact::redact(text, &refs)
    }

    async fn run_docker(&self, args: &[String], timeout: Duration) -> Result<std::process::Output> {
        let mut cmd = tokio::process::Command::new(&self.docker_bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn()?;
        // `kill_on_drop(true)` above guarantees the docker CLI is killed if
        // the timeout drops this future; the daemon-side operation is then
        // reported as a cancellation with its limit, never as success.
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;
        match result {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(err)) => Err(ControlPlaneError::Io(err)),
            Err(_) => Err(ControlPlaneError::Execution(format!(
                "docker {} cancelled after {}s timeout",
                args.first().map(String::as_str).unwrap_or("<unknown>"),
                timeout.as_secs()
            ))),
        }
    }

    async fn inspect_host_port(&self, container_id: &str) -> Option<u16> {
        let output = self
            .run_docker(
                &["port".to_string(), container_id.to_string()],
                Duration::from_secs(15),
            )
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // `8080/tcp -> 127.0.0.1:49153`: take the trailing host port.
        text.rsplit(':').next()?.trim().parse().ok()
    }
}

#[async_trait]
impl ContainerExecutor for DockerExecutor {
    async fn check_available(&self) -> RuntimeAvailability {
        let mut cmd = tokio::process::Command::new(&self.docker_bin);
        cmd.arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                RuntimeAvailability::Available {
                    docker_version: if version.is_empty() {
                        "docker (version unknown)".to_string()
                    } else {
                        version
                    },
                }
            }
            Ok(Ok(output)) => RuntimeAvailability::Unavailable {
                reason: format!(
                    "docker --version exited unsuccessfully: {}",
                    self.redact_text(&String::from_utf8_lossy(&output.stderr))
                ),
            },
            Ok(Err(err)) => RuntimeAvailability::Unavailable {
                reason: format!("docker binary could not be executed: {err}"),
            },
            Err(_) => RuntimeAvailability::Unavailable {
                reason: "docker --version timed out after 10s".to_string(),
            },
        }
    }

    async fn build(
        &self,
        dockerfile: &Path,
        context: &Path,
        tag: &str,
        limits: &labrys_core::SandboxLimits,
        identity: &ExecutionIdentity,
    ) -> BuildResult {
        if let Err(err) = limits.validate() {
            return BuildResult::failed(format!("refused unschedulable build: {err}"));
        }
        let args = match docker_build_args(dockerfile, context, tag) {
            Ok(args) => args,
            Err(err) => return BuildResult::failed(format!("refused unschedulable build: {err}")),
        };
        let timeout = Duration::from_secs(limits.timeout_secs.max(1));
        match self.run_docker(&args, timeout).await {
            Ok(output) if output.status.success() => BuildResult::succeeded(format!(
                "build {} finished for application {} (trace {})",
                tag, identity.application_id, identity.trace_id
            )),
            Ok(output) => {
                let detail = self.redact_text(&String::from_utf8_lossy(&output.stderr));
                let mut result = BuildResult::failed(format!(
                    "build {} failed for application {}: {}",
                    tag, identity.application_id, detail
                ));
                result.limit_enforced = Some(
                    EffectiveLimits {
                        cpu_millicpus: limits.cpu_millicpus,
                        memory_mb: limits.memory_mb,
                        timeout_secs: limits.timeout_secs,
                        readonly_root: limits.readonly_root,
                        network: format!("{:?}", limits.network).to_lowercase(),
                        drop_capabilities: limits.drop_capabilities.clone(),
                        max_processes: limits.max_processes,
                        port_exposed: !matches!(limits.network, labrys_core::NetworkMode::Disabled),
                    }
                    .summary(),
                );
                result.recovery =
                    Some("inspect the build log, fix the Dockerfile, then retry".to_string());
                result
            }
            Err(ControlPlaneError::Execution(reason)) if reason.contains("cancelled after") => {
                BuildResult::timed_out(limits.timeout_secs, limits.timeout_secs + 1)
            }
            Err(err) => BuildResult::failed(format!("build could not start: {err}")),
        }
    }

    async fn run(
        &self,
        config: &RuntimeConfig,
        image: &str,
        container_name: &str,
        _identity: &ExecutionIdentity,
    ) -> Result<RunningContainer> {
        let args = docker_run_args(config, container_name, image)?;
        let timeout = Duration::from_secs(120);
        let output = self.run_docker(&args, timeout).await?;
        if !output.status.success() {
            return Err(ControlPlaneError::Execution(self.redact_text(&format!(
                "docker run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut container = RunningContainer::new(id, container_name)?;
        container.host_port = self.inspect_host_port(&container.id).await;
        Ok(container)
    }

    async fn stop(&self, container_id: &str) -> Result<()> {
        validate_container_id(container_id)?;
        let output = self
            .run_docker(
                &["stop".to_string(), container_id.to_string()],
                Duration::from_secs(60),
            )
            .await?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if stderr.contains("No such container") {
            return Ok(());
        }
        Err(ControlPlaneError::Execution(
            self.redact_text(&format!("docker stop failed: {stderr}")),
        ))
    }

    async fn remove(&self, container_id: &str) -> Result<()> {
        validate_container_id(container_id)?;
        let output = self
            .run_docker(
                &["rm".to_string(), "-f".to_string(), container_id.to_string()],
                Duration::from_secs(60),
            )
            .await?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if stderr.contains("No such container") {
            return Ok(());
        }
        Err(ControlPlaneError::Execution(
            self.redact_text(&format!("docker rm failed: {stderr}")),
        ))
    }

    async fn logs(&self, container_id: &str, tail: usize) -> Result<String> {
        validate_container_id(container_id)?;
        let output = self
            .run_docker(
                &[
                    "logs".to_string(),
                    "--tail".to_string(),
                    tail.max(1).to_string(),
                    container_id.to_string(),
                ],
                Duration::from_secs(30),
            )
            .await?;
        if !output.status.success() {
            return Err(ControlPlaneError::Execution(self.redact_text(&format!(
                "docker logs failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }
        let mut text = String::from_utf8_lossy(&output.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(self.redact_text(&text))
    }

    async fn health(&self, config: &RuntimeConfig, host: &str, port: u16) -> HealthStatus {
        probe_health(config, host, port).await
    }
}

/// Reports the runtime as unavailable with an environment-blocker reason.
///
/// Used when Docker is absent (or explicitly disabled) so callers surface a
/// blocker instead of simulating execution evidence.
#[derive(Debug, Clone)]
pub struct UnavailableExecutor {
    reason: String,
}

impl UnavailableExecutor {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn missing_docker() -> Self {
        Self::new("docker runtime is not available in this environment")
    }
}

#[async_trait]
impl ContainerExecutor for UnavailableExecutor {
    async fn check_available(&self) -> RuntimeAvailability {
        RuntimeAvailability::Unavailable {
            reason: self.reason.clone(),
        }
    }

    async fn build(
        &self,
        _dockerfile: &Path,
        _context: &Path,
        _tag: &str,
        limits: &labrys_core::SandboxLimits,
        _identity: &ExecutionIdentity,
    ) -> BuildResult {
        let mut result = BuildResult::failed(format!(
            "build blocked: runtime unavailable ({})",
            self.reason
        ));
        result.limit_enforced = Some(format!("timeout_secs={}", limits.timeout_secs));
        result.recovery =
            Some("provision a container runtime, then retry; no image was produced".to_string());
        result
    }

    async fn run(
        &self,
        _config: &RuntimeConfig,
        _image: &str,
        _container_name: &str,
        _identity: &ExecutionIdentity,
    ) -> Result<RunningContainer> {
        Err(ControlPlaneError::Unavailable(self.reason.clone()))
    }

    async fn stop(&self, _container_id: &str) -> Result<()> {
        Err(ControlPlaneError::Unavailable(self.reason.clone()))
    }

    async fn remove(&self, _container_id: &str) -> Result<()> {
        Err(ControlPlaneError::Unavailable(self.reason.clone()))
    }

    async fn logs(&self, _container_id: &str, _tail: usize) -> Result<String> {
        Err(ControlPlaneError::Unavailable(self.reason.clone()))
    }

    async fn health(&self, _config: &RuntimeConfig, _host: &str, _port: u16) -> HealthStatus {
        HealthStatus::Unknown {
            reason: format!("health blocked: runtime unavailable ({})", self.reason),
        }
    }
}

/// Platform-owned health probe for a started workload.
///
/// Opens a TCP connection and issues a minimal HTTP GET — to the configured
/// health path when one exists, otherwise to `/` — then parses the status
/// code. The raw signal is evaluated with the core
/// [`labrys_core::evaluate_health`] semantics, so development and production
/// health stay distinct and a dev-server 3xx can never read as production
/// healthy.
///
/// A bare TCP accept is deliberately *not* health: the container network
/// stack (e.g. Docker's userland proxy) accepts connections for published
/// ports even when nothing inside the container is listening, so an accepted
/// socket with no HTTP response reports no signal (`Unknown`, never
/// `Healthy`). Only a parseable HTTP status can establish readiness, and
/// agent claims are not an input on this path at all.
pub async fn probe_health(config: &RuntimeConfig, host: &str, port: u16) -> HealthStatus {
    if port == 0 {
        return HealthStatus::Unknown {
            reason: "health probe requires a port in 1..=65535".to_string(),
        };
    }
    if !config.limits.allows_port_exposure() {
        return HealthStatus::Unknown {
            reason: "health probe blocked: sandbox network is disabled".to_string(),
        };
    }
    let timeout = Duration::from_secs(5);
    let stream = match tokio::time::timeout(timeout, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => stream,
        _ => {
            return labrys_core::evaluate_health(config, None, None);
        }
    };
    // Path under test: the configured health path, else the conventional `/`.
    // A non-HTTP service yields no parseable status and therefore no signal.
    let path = config
        .healthcheck_path
        .clone()
        .unwrap_or_else(|| "/".to_string());
    match http_status_via(stream, host, &path).await {
        Some(status) => labrys_core::evaluate_health(config, Some(status), Some(path.as_str())),
        None => labrys_core::evaluate_health(config, None, None),
    }
}

async fn http_status_via(stream: TcpStream, host: &str, path: &str) -> Option<u16> {
    let (reader, mut writer) = stream.into_split();
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if writer.write_all(request.as_bytes()).await.is_err() {
        return None;
    }
    let _ = writer.shutdown().await;
    let mut reader = tokio::io::BufReader::new(reader);
    let mut status_line = String::new();
    use tokio::io::AsyncBufReadExt;
    let read =
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut status_line)).await;
    if read.is_err() {
        return None;
    }
    // `HTTP/1.1 200 OK` -> 200.
    status_line.split_whitespace().nth(1)?.parse().ok()
}

fn path_arg(path: &Path) -> Result<String> {
    let text = path.to_string_lossy().to_string();
    if text.trim().is_empty() {
        return Err(ControlPlaneError::Execution(
            "execution requires a non-empty path".to_string(),
        ));
    }
    Ok(text)
}

fn validate_tag(tag: &str) -> Result<()> {
    if tag.trim().is_empty() {
        return Err(ControlPlaneError::Execution(
            "container image tag must not be empty".to_string(),
        ));
    }
    if tag
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '$' | '`' | '"' | '\''))
    {
        return Err(ControlPlaneError::Execution(format!(
            "container image tag is not a plain reference: {tag}"
        )));
    }
    Ok(())
}

fn validate_container_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(ControlPlaneError::Execution(
            "container name must not be empty".to_string(),
        ));
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => {
            return Err(ControlPlaneError::Execution(format!(
                "container name must start with an alphanumeric: {name}"
            )));
        }
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(ControlPlaneError::Execution(format!(
            "container name contains an unsupported character: {name}"
        )));
    }
    Ok(())
}

fn validate_container_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(ControlPlaneError::Execution(
            "container id must not be empty".to_string(),
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
    {
        return Err(ControlPlaneError::Execution(
            "container id contains an unsupported character".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use labrys_core::{RuntimeProfile, SandboxLimits, SupportTier};

    fn dev_config() -> RuntimeConfig {
        RuntimeConfig {
            kind: labrys_core::RuntimeKind::Generic,
            tier: SupportTier::Tier0,
            profile: RuntimeProfile::Development,
            port: 8080,
            healthcheck_path: None,
            dockerfile: Some("Dockerfile".to_string()),
            run_command: vec!["docker".to_string()],
            oci_image: None,
            limits: SandboxLimits::docker_default(),
        }
    }

    #[test]
    fn run_args_carry_sandbox_limits_without_shell() {
        let args = docker_run_args(&dev_config(), "labrys-abc", "labrys-test:latest").unwrap();
        let joined = args.join(" ");
        assert!(joined.contains("--cpus 1.00"), "{joined}");
        assert!(joined.contains("--memory 512m"), "{joined}");
        assert!(joined.contains("--pids-limit 128"), "{joined}");
        assert!(joined.contains("--read-only"), "{joined}");
        assert!(joined.contains("--cap-drop ALL"), "{joined}");
        assert!(joined.contains("--network bridge"), "{joined}");
        assert!(!joined.contains("sh"), "{joined}");
        assert!(!joined.contains("&&"), "{joined}");
    }

    #[test]
    fn disabled_network_publishes_no_ports() {
        let mut config = dev_config();
        config.limits.network = labrys_core::NetworkMode::Disabled;
        let args = docker_run_args(&config, "labrys-abc", "img:latest").unwrap();
        assert!(args.contains(&"none".to_string()));
        assert!(!args.iter().any(|a| a == &"-p".to_string()));
        assert!(!config.limits.allows_port_exposure());
    }

    #[test]
    fn invalid_limits_rejected_before_scheduling() {
        let mut config = dev_config();
        config.limits.memory_mb = 0;
        assert!(enforce_limits_before_schedule(&config).is_err());
        assert!(docker_run_args(&config, "labrys-abc", "img:latest").is_err());
    }

    #[test]
    fn container_names_cannot_escape_argv() {
        assert!(validate_container_name("labrys-session-1").is_ok());
        assert!(validate_container_name("evil; rm -rf /").is_err());
        assert!(validate_container_name("evil$(id)").is_err());
        assert!(validate_container_name("../escape").is_err());
    }

    #[test]
    fn development_is_never_a_production_artifact() {
        assert!(verify_production_artifact(&dev_config()).is_err());
        let mut prod = dev_config();
        prod.profile = RuntimeProfile::Production;
        prod.oci_image = Some("oci://app:prod".to_string());
        assert_eq!(verify_production_artifact(&prod).unwrap(), "oci://app:prod");
    }

    #[test]
    fn health_maps_to_controller_phases() {
        assert_eq!(
            prospective_phase(&HealthStatus::Healthy),
            ResourcePhase::Ready
        );
        assert_eq!(
            prospective_phase(&HealthStatus::Starting),
            ResourcePhase::Provisioning
        );
        assert_eq!(
            prospective_phase(&HealthStatus::Unhealthy {
                reason: "x".to_string()
            }),
            ResourcePhase::Degraded
        );
    }

    #[tokio::test]
    async fn unavailable_executor_blocks_without_simulation() {
        let executor = UnavailableExecutor::missing_docker();
        assert!(!executor.check_available().await.is_available());
        let identity = ExecutionIdentity::new("app", "trace").unwrap();
        let result = executor
            .build(
                Path::new("Dockerfile"),
                Path::new("."),
                "img:latest",
                &SandboxLimits::docker_default(),
                &identity,
            )
            .await;
        assert!(!result.is_success());
        assert!(result.recovery.is_some());
        let health = executor.health(&dev_config(), "127.0.0.1", 8080).await;
        assert!(!health.is_healthy());
    }
}
