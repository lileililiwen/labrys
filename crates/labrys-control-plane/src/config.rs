use std::env;
use std::path::PathBuf;

use crate::error::{ControlPlaneError, Result};

/// How the daemon selects its job dispatcher at startup.
///
/// `Auto` probes the container runtime and runs real execution when it is
/// reachable, falling back to a no-op dispatcher that reports an environment
/// blocker. `Docker` requires the runtime and fails startup when it is
/// unreachable. `Disabled` never probes and always reports the blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeMode {
    #[default]
    Auto,
    Docker,
    Disabled,
}

impl RuntimeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Docker => "docker",
            Self::Disabled => "disabled",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "docker" => Ok(Self::Docker),
            "disabled" => Ok(Self::Disabled),
            other => Err(ControlPlaneError::Config(format!(
                "LABRYS_RUNTIME_MODE must be one of auto, docker, disabled (got '{other}')"
            ))),
        }
    }
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Default preview time-to-live: one hour.
pub const DEFAULT_PREVIEW_TTL_SECS: u64 = 3_600;
/// Bounds for `LABRYS_PREVIEW_TTL_SECONDS`: one minute to one day.
pub const MIN_PREVIEW_TTL_SECS: u64 = 60;
pub const MAX_PREVIEW_TTL_SECS: u64 = 86_400;

/// Runtime configuration for the control-plane process.
///
/// Database credentials come from process configuration (environment), never
/// from a `labrys.yaml` manifest or an agent prompt. All timing fields are
/// bounded so a misconfiguration cannot produce an unbounded retry or a
/// never-expiring lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// PostgreSQL connection URL.
    pub database_url: String,
    /// Stable identity attributed to every claim/attempt/terminal transition.
    pub worker_id: String,
    /// Seconds a claimed job lease stays valid before another worker may
    /// recover it.
    pub lease_seconds: i64,
    /// Milliseconds a worker sleeps between claim polls when idle.
    pub poll_interval_ms: u64,
    /// Upper bound on the in-flight drain during graceful shutdown.
    pub drain_timeout_ms: u64,
    /// Maximum jobs a single worker runs concurrently.
    pub max_concurrency: usize,
    /// Whether to run embedded migrations at startup.
    pub run_migrations: bool,
    /// How the daemon selects its job dispatcher (auto/docker/disabled).
    pub runtime_mode: RuntimeMode,
    /// Container runtime binary the daemon probes and executes through.
    pub docker_bin: PathBuf,
    /// Host root under which session worktrees are materialized.
    pub workspace_root: PathBuf,
    /// Preview time-to-live in seconds, bounded to one minute through one day.
    pub preview_ttl_secs: u64,
}

impl Config {
    /// Loads configuration from the process environment.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Loads configuration from an arbitrary lookup, so tests can drive every
    /// default and validation path without touching the real environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let database_url = lookup("LABRYS_DATABASE_URL")
            .or_else(|| lookup("DATABASE_URL"))
            .unwrap_or_default();
        let worker_id = lookup("LABRYS_WORKER_ID").unwrap_or_else(default_worker_id);
        let lease_seconds = parse_i64(&lookup, "LABRYS_LEASE_SECONDS", 60)?;
        let poll_interval_ms = parse_u64(&lookup, "LABRYS_POLL_INTERVAL_MS", 250)?;
        let drain_timeout_ms = parse_u64(&lookup, "LABRYS_DRAIN_TIMEOUT_MS", 5_000)?;
        let max_concurrency = parse_usize(&lookup, "LABRYS_MAX_CONCURRENCY", 1)?;
        let run_migrations = match lookup("LABRYS_RUN_MIGRATIONS") {
            Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
            None => true,
        };
        let runtime_mode = match lookup("LABRYS_RUNTIME_MODE") {
            Some(v) => RuntimeMode::parse(&v)?,
            None => RuntimeMode::default(),
        };
        let docker_bin = match lookup("LABRYS_DOCKER_BIN") {
            Some(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
            Some(_) => {
                return Err(ControlPlaneError::Config(
                    "LABRYS_DOCKER_BIN must not be empty".to_string(),
                ));
            }
            None => PathBuf::from("docker"),
        };
        let workspace_root = match lookup("LABRYS_WORKSPACE_ROOT") {
            Some(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
            Some(_) => {
                return Err(ControlPlaneError::Config(
                    "LABRYS_WORKSPACE_ROOT must not be empty".to_string(),
                ));
            }
            None => default_workspace_root(),
        };
        let preview_ttl_secs = parse_u64(
            &lookup,
            "LABRYS_PREVIEW_TTL_SECONDS",
            DEFAULT_PREVIEW_TTL_SECS,
        )?;
        let config = Self {
            database_url,
            worker_id,
            lease_seconds,
            poll_interval_ms,
            drain_timeout_ms,
            max_concurrency,
            run_migrations,
            runtime_mode,
            docker_bin,
            workspace_root,
            preview_ttl_secs,
        };
        config.validate()?;
        Ok(config)
    }

    /// Rejects unsafe or unbounded settings before any connection is opened.
    pub fn validate(&self) -> Result<()> {
        let url = self.database_url.trim();
        if url.is_empty() {
            return Err(ControlPlaneError::Config(
                "database url is required (LABRYS_DATABASE_URL/DATABASE_URL)".to_string(),
            ));
        }
        if !(url.starts_with("postgres://")
            || url.starts_with("postgresql://")
            || url.starts_with("postgres+"))
        {
            return Err(ControlPlaneError::Config(
                "database url must be a postgresql:// connection string".to_string(),
            ));
        }
        if self.worker_id.trim().is_empty() {
            return Err(ControlPlaneError::Config(
                "worker id must not be empty".to_string(),
            ));
        }
        if self.lease_seconds < 1 || self.lease_seconds > 3_600 {
            return Err(ControlPlaneError::Config(
                "lease seconds must be between 1 and 3600".to_string(),
            ));
        }
        if self.poll_interval_ms < 10 || self.poll_interval_ms > 60_000 {
            return Err(ControlPlaneError::Config(
                "poll interval must be between 10 and 60000 ms".to_string(),
            ));
        }
        if self.drain_timeout_ms < 100 || self.drain_timeout_ms > 120_000 {
            return Err(ControlPlaneError::Config(
                "drain timeout must be between 100 and 120000 ms".to_string(),
            ));
        }
        if self.max_concurrency < 1 || self.max_concurrency > 64 {
            return Err(ControlPlaneError::Config(
                "max concurrency must be between 1 and 64".to_string(),
            ));
        }
        if self.docker_bin.as_os_str().is_empty() {
            return Err(ControlPlaneError::Config(
                "docker binary path must not be empty".to_string(),
            ));
        }
        if self.workspace_root.as_os_str().is_empty() {
            return Err(ControlPlaneError::Config(
                "workspace root must not be empty".to_string(),
            ));
        }
        if self.preview_ttl_secs < MIN_PREVIEW_TTL_SECS
            || self.preview_ttl_secs > MAX_PREVIEW_TTL_SECS
        {
            return Err(ControlPlaneError::Config(format!(
                "preview ttl must be between {MIN_PREVIEW_TTL_SECS} and {MAX_PREVIEW_TTL_SECS} seconds"
            )));
        }
        Ok(())
    }
}

fn default_workspace_root() -> PathBuf {
    std::env::temp_dir().join("labrys-workspaces")
}

fn default_worker_id() -> String {
    format!("worker-{}", uuid::Uuid::new_v4().simple())
}

fn parse_i64(lookup: &impl Fn(&str) -> Option<String>, key: &str, default: i64) -> Result<i64> {
    parse_value(lookup, key, default, |v| v.parse::<i64>().ok())
}

fn parse_u64(lookup: &impl Fn(&str) -> Option<String>, key: &str, default: u64) -> Result<u64> {
    parse_value(lookup, key, default, |v| v.parse::<u64>().ok())
}

fn parse_usize(
    lookup: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: usize,
) -> Result<usize> {
    parse_value(lookup, key, default, |v| v.parse::<usize>().ok())
}

fn parse_value<T: Copy>(
    lookup: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: T,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<T> {
    match lookup(key) {
        None => Ok(default),
        Some(raw) => parse(raw.trim()).ok_or_else(|| {
            ControlPlaneError::Config(format!("{key} is not a valid integer: {raw}"))
        }),
    }
}
