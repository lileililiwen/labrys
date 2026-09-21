//! Deterministic runtime execution model and Docker sandbox policy.
//!
//! Covers requirement `runtime-platform`: generic-container compatibility as
//! the minimum boundary, explicit development/production profiles, and
//! enforced sandbox limits. This module is pure and deterministic: it plans
//! builds, validates sandbox limits, resolves commands per profile, and
//! evaluates health semantics. It never talks to a Docker daemon; actual
//! execution belongs to infrastructure built on these plans.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::inspector::ProjectSnapshot;

/// Language-independent runtime identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeKind {
    Generic,
    Node,
    DotNet,
    Python,
    Rust,
    Expo,
}

impl RuntimeKind {
    /// Human-readable name used in plans and events.
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Node => "node",
            Self::DotNet => "dotnet",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Expo => "expo",
        }
    }
}

/// Runtime support tier (ROADMAP runtime support tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SupportTier {
    Tier0,
    Tier1,
    Tier2,
    Tier3,
}

/// Explicit execution profile. A dev server is never a production command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeProfile {
    Development,
    Production,
}

/// Liveness of a running workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum HealthStatus {
    Starting,
    Healthy,
    Unhealthy { reason: String },
    Unknown { reason: String },
}

impl HealthStatus {
    /// True only for verified-healthy workloads.
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// Reachable network endpoint for a started workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// False when the sandbox exposes no port (e.g. network disabled).
    pub exposed: bool,
}

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16, exposed: bool) -> Result<Self> {
        if port == 0 {
            return Err(CoreError::InvalidRuntime(
                "port must be in 1..=65535".to_string(),
            ));
        }
        Ok(Self {
            host: host.into(),
            port,
            exposed,
        })
    }

    /// Loopback endpoint for a sandbox-exposed container port.
    pub fn loopback(port: u16, exposed: bool) -> Result<Self> {
        Self::new("127.0.0.1", port, exposed)
    }
}

/// Sandbox network posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkMode {
    /// No network access at all.
    Disabled,
    /// Isolated bridge with port exposure but no host network.
    Isolated,
    /// Shared bridge (default Docker behavior, still port-mapped).
    Bridged,
}

/// Enforced execution limits for one build or run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    /// CPU shares in millicpus (1000 = one vCPU).
    pub cpu_millicpus: u32,
    /// Memory cap in MiB.
    pub memory_mb: u32,
    /// Wall-clock timeout in seconds.
    pub timeout_secs: u64,
    /// When true the root filesystem is mounted read-only.
    pub readonly_root: bool,
    pub network: NetworkMode,
    /// Linux capabilities dropped (e.g. `ALL` plus explicit adds elsewhere).
    #[serde(default)]
    pub drop_capabilities: Vec<String>,
    /// Maximum process count (`pids-limit`).
    pub max_processes: u32,
}

impl SandboxLimits {
    /// Conservative Docker sandbox default used by the generic profile.
    pub fn docker_default() -> Self {
        Self {
            cpu_millicpus: 1000,
            memory_mb: 512,
            timeout_secs: 600,
            readonly_root: true,
            network: NetworkMode::Isolated,
            drop_capabilities: vec!["ALL".to_string()],
            max_processes: 128,
        }
    }

    /// Rejects nonsensical limits before anything is scheduled.
    pub fn validate(&self) -> Result<()> {
        if self.cpu_millicpus == 0 {
            return Err(CoreError::InvalidRuntime(
                "sandbox cpu_millicpus must be > 0".to_string(),
            ));
        }
        if self.memory_mb == 0 {
            return Err(CoreError::InvalidRuntime(
                "sandbox memory_mb must be > 0".to_string(),
            ));
        }
        if self.timeout_secs == 0 {
            return Err(CoreError::InvalidRuntime(
                "sandbox timeout_secs must be > 0".to_string(),
            ));
        }
        if self.max_processes == 0 {
            return Err(CoreError::InvalidRuntime(
                "sandbox max_processes must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    /// True when a port can be reached from outside the sandbox.
    pub fn allows_port_exposure(&self) -> bool {
        !matches!(self.network, NetworkMode::Disabled)
    }
}

/// Planned runtime configuration for one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub kind: RuntimeKind,
    pub tier: SupportTier,
    pub profile: RuntimeProfile,
    /// Container port the workload listens on.
    pub port: u16,
    /// Optional HTTP health path (e.g. `/healthz`). `None` means TCP-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthcheck_path: Option<String>,
    /// Dockerfile path backing generic-container builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    /// Command(s) the profile executes.
    pub run_command: Vec<String>,
    /// OCI image reference for production interchange (`None` in development).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oci_image: Option<String>,
    pub limits: SandboxLimits,
}

impl RuntimeConfig {
    /// Canonical JSON serialization.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    /// Strict JSON deserialization.
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }

    /// Endpoint for this config under its sandbox network posture.
    pub fn endpoint(&self) -> Result<Endpoint> {
        Endpoint::loopback(self.port, self.limits.allows_port_exposure())
    }
}

/// Outcome of one (simulated) build under sandbox limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum BuildStatus {
    Succeeded,
    Failed,
    Cancelled,
}

/// Deterministic build result. Timeouts cancel and mark failed, always
/// carrying the enforced limit and a recovery hint for events/logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildResult {
    pub status: BuildStatus,
    /// True when the sandbox cancelled the build (timeout or limit breach).
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_enforced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    pub detail: String,
}

impl BuildResult {
    pub fn succeeded(detail: impl Into<String>) -> Self {
        Self {
            status: BuildStatus::Succeeded,
            cancelled: false,
            limit_enforced: None,
            recovery: None,
            detail: detail.into(),
        }
    }

    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: BuildStatus::Failed,
            cancelled: false,
            limit_enforced: None,
            recovery: None,
            detail: detail.into(),
        }
    }

    /// Timeout cancellation: cancelled AND failed, with limit + recovery.
    pub fn timed_out(timeout_secs: u64, elapsed_secs: u64) -> Self {
        Self {
            status: BuildStatus::Cancelled,
            cancelled: true,
            limit_enforced: Some(format!("timeout_secs={timeout_secs}")),
            recovery: Some(
                "increase sandbox timeout_secs or reduce build work, then retry".to_string(),
            ),
            detail: format!(
                "build exceeded timeout: elapsed {elapsed_secs}s > limit {timeout_secs}s"
            ),
        }
    }

    /// Generic limit-breach cancellation (CPU, memory, pids, filesystem, ...).
    pub fn limit_cancelled(limit: impl Into<String>, detail: impl Into<String>) -> Self {
        let limit = limit.into();
        Self {
            status: BuildStatus::Cancelled,
            cancelled: true,
            limit_enforced: Some(limit),
            recovery: Some(
                "raise the enforced sandbox limit or reduce usage, then retry".to_string(),
            ),
            detail: detail.into(),
        }
    }

    pub fn is_success(&self) -> bool {
        self.status == BuildStatus::Succeeded
    }
}

/// What the detector found: kind, tier, and the evidence behind the choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectedRuntime {
    pub kind: RuntimeKind,
    pub tier: SupportTier,
    /// Deep framework identity (`django`, `fastapi`, `axum`, `rust`,
    /// `expo`) when Tier 3 layout evidence resolved one; `None` for
    /// Tier 0–2 language-level detections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// Entry point backing convention commands: the Django settings
    /// package (for `<pkg>.wsgi:application`) or the FastAPI module
    /// (for `<module>:app`). `None` when Tier 3 needs no entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prod_entry: Option<String>,
    /// Evidenced migration runner (e.g. `manage.py migrate`,
    /// `alembic upgrade head`). Empty means no runner was evidenced —
    /// the platform reports "no detected convention" rather than
    /// inventing one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrate_command: Vec<String>,
    /// Evidenced test runner. Empty means no runner was evidenced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_command: Vec<String>,
    /// Evidence sources consulted, newest last (file paths).
    #[serde(default)]
    pub evidence: Vec<String>,
}

impl DetectedRuntime {
    /// Language-level detection without deep framework identity.
    pub fn new(kind: RuntimeKind, tier: SupportTier, evidence: Vec<String>) -> Self {
        Self {
            kind,
            tier,
            framework: None,
            prod_entry: None,
            migrate_command: Vec::new(),
            test_command: Vec::new(),
            evidence,
        }
    }

    /// Tier 3 detection with deep framework identity and conventions.
    pub fn with_framework(
        kind: RuntimeKind,
        framework: impl Into<String>,
        evidence: Vec<String>,
    ) -> Self {
        Self {
            kind,
            tier: SupportTier::Tier3,
            framework: Some(framework.into()),
            prod_entry: None,
            migrate_command: Vec::new(),
            test_command: Vec::new(),
            evidence,
        }
    }

    /// Resolves the Tier 3 conventions for this detection, if any.
    pub fn conventions(&self) -> Option<FrameworkConventions> {
        let framework = self.framework.as_deref()?;
        let entry = self.prod_entry.as_deref().unwrap_or("");
        FrameworkConventions::lookup(self.kind, framework, entry).map(|conventions| {
            conventions
                .with_migrate(self.migrate_command.clone())
                .with_test(self.test_command.clone())
        })
    }
}

/// Deep Tier 3 conventions for one framework: layout-derived migration,
/// test, dev-server, production, and health defaults.
///
/// Commands are *proposed* plans, never auto-executed: the platform runs
/// them only through reviewed jobs under approval gates. An empty
/// `migrate_command`/`test_command` means no runner was evidenced — the
/// platform reports "no detected convention" rather than inventing one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameworkConventions {
    pub framework: String,
    #[serde(default)]
    pub migrate_command: Vec<String>,
    #[serde(default)]
    pub test_command: Vec<String>,
    pub dev_command: Vec<String>,
    pub prod_command: Vec<String>,
    /// Conventional HTTP health path, if the framework defines one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    /// Evidence sources behind the conventions (file paths).
    #[serde(default)]
    pub evidence: Vec<String>,
}

impl FrameworkConventions {
    /// Tier 3 convention table. `prod_entry` fills the `{entry}` slot for
    /// entry-point-derived production commands (FastAPI module path,
    /// Django WSGI module).
    pub fn lookup(kind: RuntimeKind, framework: &str, prod_entry: &str) -> Option<Self> {
        let entry = prod_entry.trim();
        match (kind, framework) {
            (RuntimeKind::Python, "django") if !entry.is_empty() => Some(Self {
                framework: "django".to_string(),
                migrate_command: vec![
                    "python".to_string(),
                    "manage.py".to_string(),
                    "migrate".to_string(),
                ],
                test_command: vec![
                    "python".to_string(),
                    "manage.py".to_string(),
                    "test".to_string(),
                ],
                dev_command: vec![
                    "python".to_string(),
                    "manage.py".to_string(),
                    "runserver".to_string(),
                ],
                prod_command: vec!["gunicorn".to_string(), format!("{entry}.wsgi:application")],
                health_path: None,
                evidence: Vec::new(),
            }),
            (RuntimeKind::Python, "fastapi") if !entry.is_empty() => Some(Self {
                framework: "fastapi".to_string(),
                migrate_command: Vec::new(),
                test_command: Vec::new(),
                dev_command: vec![
                    "python".to_string(),
                    "-m".to_string(),
                    "uvicorn".to_string(),
                    format!("{entry}:app"),
                    "--reload".to_string(),
                ],
                prod_command: vec![
                    "python".to_string(),
                    "-m".to_string(),
                    "uvicorn".to_string(),
                    format!("{entry}:app"),
                ],
                health_path: Some("/health".to_string()),
                evidence: Vec::new(),
            }),
            (RuntimeKind::Rust, "axum") => Some(Self {
                framework: "axum".to_string(),
                migrate_command: Vec::new(),
                test_command: vec!["cargo".to_string(), "test".to_string()],
                dev_command: vec!["cargo".to_string(), "run".to_string()],
                prod_command: vec![
                    "cargo".to_string(),
                    "build".to_string(),
                    "--release".to_string(),
                ],
                health_path: Some("/health".to_string()),
                evidence: Vec::new(),
            }),
            (RuntimeKind::Rust, "rust") => Some(Self {
                framework: "rust".to_string(),
                migrate_command: Vec::new(),
                test_command: vec!["cargo".to_string(), "test".to_string()],
                dev_command: vec!["cargo".to_string(), "run".to_string()],
                prod_command: vec![
                    "cargo".to_string(),
                    "build".to_string(),
                    "--release".to_string(),
                ],
                health_path: None,
                evidence: Vec::new(),
            }),
            (RuntimeKind::Expo, "expo") => Some(Self {
                framework: "expo".to_string(),
                migrate_command: Vec::new(),
                test_command: Vec::new(),
                dev_command: vec!["expo".to_string(), "start".to_string()],
                prod_command: vec!["expo".to_string(), "export".to_string()],
                health_path: None,
                evidence: Vec::new(),
            }),
            _ => None,
        }
    }

    /// Fills migration/test conventions evidenced after lookup.
    pub fn with_migrate(mut self, command: Vec<String>) -> Self {
        self.migrate_command = command;
        self
    }

    pub fn with_test(mut self, command: Vec<String>) -> Self {
        self.test_command = command;
        self
    }
}

/// Language-independent runtime execution contract.
///
/// Adapters detect, prepare (plan a [`RuntimeConfig`]), and evaluate
/// simulated build/health outcomes. Real Docker/container I/O lives outside
/// `labrys-core`; the trait keeps planning deterministic and testable.
pub trait RuntimeAdapter {
    fn kind(&self) -> RuntimeKind;
    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }
    /// Detects support from a snapshot; `None` means "not mine".
    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime>;
    fn dev_command(&self) -> Vec<String>;
    fn prod_command(&self) -> Vec<String>;
    /// Production interchange boundary: OCI image except dev-only flows.
    fn prod_oci_image(&self) -> Option<String> {
        None
    }
    /// Default HTTP health path for this runtime, if any.
    fn health_path(&self) -> Option<String> {
        None
    }
}

/// Generic container adapter: the Tier 0 compatibility floor.
///
/// Any project with a valid Dockerfile (a `Dockerfile*` file containing a
/// `FROM` instruction), configured port, and optional healthcheck is eligible
/// for build, run, preview, logs, and deployment workflows.
pub struct GenericAdapter;

impl GenericAdapter {
    /// True when the snapshot carries a plausible Dockerfile.
    pub fn has_dockerfile(snapshot: &ProjectSnapshot) -> bool {
        snapshot
            .files_named_anywhere(&["Dockerfile", "dockerfile"])
            .iter()
            .any(|(_, content)| {
                content
                    .lines()
                    .any(|l| l.trim_start().to_ascii_uppercase().starts_with("FROM "))
            })
    }

    fn dockerfile_path(snapshot: &ProjectSnapshot) -> Option<String> {
        let mut found = snapshot.files_named_anywhere(&["Dockerfile", "dockerfile"]);
        found.sort_by(|a, b| a.0.cmp(b.0));
        found.first().map(|(p, _)| (*p).to_string())
    }
}

impl RuntimeAdapter for GenericAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Generic
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier0
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        let path = Self::dockerfile_path(snapshot)?;
        if !Self::has_dockerfile(snapshot) {
            return None;
        }
        Some(DetectedRuntime::new(
            RuntimeKind::Generic,
            SupportTier::Tier0,
            vec![path],
        ))
    }

    fn dev_command(&self) -> Vec<String> {
        vec!["docker".to_string(), "build".to_string(), ".".to_string()]
    }

    fn prod_command(&self) -> Vec<String> {
        vec!["docker".to_string(), "build".to_string(), ".".to_string()]
    }

    fn prod_oci_image(&self) -> Option<String> {
        Some("oci://app:prod".to_string())
    }
}

/// Node.js adapter (Tier 1; Tier 2 with framework layout).
pub struct NodeAdapter;

impl RuntimeAdapter for NodeAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Node
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        if !snapshot.has_file("package.json") {
            return None;
        }
        let mut evidence = vec!["package.json".to_string()];
        let tier = if !snapshot
            .files_named_anywhere(&["next.config.js", "next.config.mjs", "next.config.ts"])
            .is_empty()
        {
            evidence.push("next.config.*".to_string());
            SupportTier::Tier2
        } else {
            SupportTier::Tier1
        };
        Some(DetectedRuntime::new(RuntimeKind::Node, tier, evidence))
    }

    fn dev_command(&self) -> Vec<String> {
        vec!["npm".to_string(), "run".to_string(), "dev".to_string()]
    }

    fn prod_command(&self) -> Vec<String> {
        vec![
            "npm".to_string(),
            "run".to_string(),
            "build".to_string(),
            "&&".to_string(),
            "npm".to_string(),
            "start".to_string(),
        ]
    }

    fn prod_oci_image(&self) -> Option<String> {
        Some("oci://node-app:prod".to_string())
    }

    fn health_path(&self) -> Option<String> {
        Some("/".to_string())
    }
}

/// .NET adapter (Tier 1; Tier 2 with ASP.NET Core web conventions).
pub struct DotNetAdapter;

impl DotNetAdapter {
    fn has_project(snapshot: &ProjectSnapshot) -> bool {
        !snapshot.files_with_extension(".csproj").is_empty()
            || !snapshot.files_with_extension(".sln").is_empty()
    }
}

impl RuntimeAdapter for DotNetAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::DotNet
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        if !Self::has_project(snapshot) {
            return None;
        }
        let mut evidence = vec!["*.csproj".to_string()];
        let tier = if snapshot.has_file("appsettings.json")
            || snapshot
                .any_content_contains_insensitive("Microsoft.AspNetCore")
                .is_some()
        {
            evidence.push("appsettings.json|Microsoft.AspNetCore".to_string());
            SupportTier::Tier2
        } else {
            SupportTier::Tier1
        };
        Some(DetectedRuntime::new(RuntimeKind::DotNet, tier, evidence))
    }

    fn dev_command(&self) -> Vec<String> {
        vec!["dotnet".to_string(), "watch".to_string()]
    }

    fn prod_command(&self) -> Vec<String> {
        vec![
            "dotnet".to_string(),
            "publish".to_string(),
            "-c".to_string(),
            "Release".to_string(),
        ]
    }

    fn prod_oci_image(&self) -> Option<String> {
        Some("oci://dotnet-app:prod".to_string())
    }

    fn health_path(&self) -> Option<String> {
        Some("/healthz".to_string())
    }
}

/// Tier 3 Django: `manage.py` plus a settings module inside a package
/// plus a migrations layout, so the WSGI entry and both runners resolve.
/// Flat layouts (root `settings.py`) stay Tier 2: the WSGI module cannot
/// be derived without inventing it.
fn detect_django(snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
    let mut settings = snapshot.files_named_anywhere(&["settings.py"]);
    settings.sort_by(|a, b| a.0.cmp(b.0));
    let (settings_path, _) = settings.first()?;
    // The settings package backs `<pkg>.wsgi:application`.
    let package = settings_path
        .rsplit_once('/')
        .map(|(dir, _)| dir.replace('/', "."))
        .filter(|dir| !dir.is_empty())?;
    if !snapshot.has_dir_anywhere("migrations") {
        return None;
    }
    let mut detected = DetectedRuntime::with_framework(
        RuntimeKind::Python,
        "django",
        vec![
            "manage.py".to_string(),
            (*settings_path).to_string(),
            "migrations/".to_string(),
        ],
    );
    detected.prod_entry = Some(package);
    detected.migrate_command = vec![
        "python".to_string(),
        "manage.py".to_string(),
        "migrate".to_string(),
    ];
    detected.test_command = vec![
        "python".to_string(),
        "manage.py".to_string(),
        "test".to_string(),
    ];
    Some(detected)
}

/// Tier 3 FastAPI: an app entry instantiating `FastAPI` plus runner
/// tooling (uvicorn, alembic, or pytest/tests). The module path backs
/// `<module>:app` for uvicorn.
fn detect_fastapi(snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
    let mut entries = snapshot.files_named_anywhere(&["main.py", "app.py"]);
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let (entry_path, _) = entries
        .iter()
        .find(|(_, content)| content.contains("FastAPI"))?;
    let module = entry_path
        .strip_suffix(".py")
        .unwrap_or(entry_path)
        .replace('/', ".");
    let has_uvicorn = snapshot
        .any_content_contains_insensitive("uvicorn")
        .is_some();
    let has_alembic = snapshot.has_file("alembic.ini") || snapshot.has_dir_anywhere("alembic");
    let has_pytest = snapshot
        .any_content_contains_insensitive("pytest")
        .is_some()
        || snapshot.has_dir_anywhere("tests");
    if !(has_uvicorn || has_alembic || has_pytest) {
        return None;
    }
    let mut evidence = vec![
        "requirements.txt|pyproject.toml|setup.py".to_string(),
        (*entry_path).to_string(),
    ];
    if has_alembic {
        evidence.push("alembic.ini|alembic/".to_string());
    }
    let mut detected = DetectedRuntime::with_framework(RuntimeKind::Python, "fastapi", evidence);
    detected.prod_entry = Some(module);
    if has_alembic {
        detected.migrate_command = vec![
            "alembic".to_string(),
            "upgrade".to_string(),
            "head".to_string(),
        ];
    }
    if has_pytest {
        detected.test_command = vec!["pytest".to_string()];
    }
    Some(detected)
}

/// Python adapter (Tier 1; Tier 2 with Django/FastAPI conventions;
/// Tier 3 with deep Django/FastAPI layouts).
pub struct PythonAdapter;

impl RuntimeAdapter for PythonAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Python
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        let has_manifest = snapshot.has_file("requirements.txt")
            || snapshot.has_file("pyproject.toml")
            || snapshot.has_file("setup.py");
        if !has_manifest {
            return None;
        }
        // Tier 3 Django: manage.py plus a settings module plus a
        // migrations layout, so the WSGI entry and runners resolve.
        if snapshot.has_file("manage.py") {
            if let Some(django) = detect_django(snapshot) {
                return Some(django);
            }
        }
        // Tier 3 FastAPI: an app entry instantiating FastAPI plus runner
        // tooling (uvicorn, alembic, or pytest/tests).
        if snapshot
            .any_content_contains_insensitive("fastapi")
            .is_some()
        {
            if let Some(fastapi) = detect_fastapi(snapshot) {
                return Some(fastapi);
            }
        }
        let mut evidence = vec!["requirements.txt|pyproject.toml|setup.py".to_string()];
        let tier = if snapshot.has_file("manage.py")
            || snapshot
                .any_content_contains_insensitive("fastapi")
                .is_some()
            || snapshot
                .any_content_contains_insensitive("django")
                .is_some()
        {
            evidence.push("manage.py|fastapi|django".to_string());
            SupportTier::Tier2
        } else {
            SupportTier::Tier1
        };
        Some(DetectedRuntime::new(RuntimeKind::Python, tier, evidence))
    }

    fn dev_command(&self) -> Vec<String> {
        vec![
            "python".to_string(),
            "-m".to_string(),
            "uvicorn".to_string(),
            "app:app".to_string(),
            "--reload".to_string(),
        ]
    }

    fn prod_command(&self) -> Vec<String> {
        vec![
            "python".to_string(),
            "-m".to_string(),
            "gunicorn".to_string(),
            "app:app".to_string(),
        ]
    }

    fn prod_oci_image(&self) -> Option<String> {
        Some("oci://python-app:prod".to_string())
    }

    fn health_path(&self) -> Option<String> {
        Some("/healthz".to_string())
    }
}

/// Rust adapter (Tier 1; Tier 2 with web-framework dependencies).
pub struct RustAdapter;

impl RuntimeAdapter for RustAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Rust
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        let manifest = snapshot.get("Cargo.toml")?;
        // Tier 3 Axum: the dependency plus a binary entry plus migration
        // tooling (sqlx/diesel markers or a migrations layout).
        if manifest.contains("axum") && snapshot.has_file("src/main.rs") {
            let tooling = manifest.contains("sqlx")
                || manifest.contains("diesel")
                || snapshot.has_dir_anywhere("migrations")
                || snapshot.has_file("diesel.toml");
            if tooling {
                let mut detected = DetectedRuntime::with_framework(
                    RuntimeKind::Rust,
                    "axum",
                    vec!["Cargo.toml:axum".to_string(), "src/main.rs".to_string()],
                );
                if manifest.contains("sqlx") {
                    detected.migrate_command =
                        vec!["sqlx".to_string(), "migrate".to_string(), "run".to_string()];
                    detected.evidence.push("Cargo.toml:sqlx".to_string());
                } else if manifest.contains("diesel") || snapshot.has_file("diesel.toml") {
                    detected.migrate_command = vec![
                        "diesel".to_string(),
                        "migration".to_string(),
                        "run".to_string(),
                    ];
                    detected
                        .evidence
                        .push("Cargo.toml:diesel|diesel.toml".to_string());
                } else {
                    detected.evidence.push("migrations/".to_string());
                }
                detected.test_command = vec!["cargo".to_string(), "test".to_string()];
                return Some(detected);
            }
        }
        // Tier 3 Rust workspace: workspace layout plus sqlx/diesel plus a
        // migrations layout for the deep migration/test conventions.
        if manifest.contains("[workspace]")
            && (manifest.contains("sqlx") || manifest.contains("diesel"))
            && snapshot.has_dir_anywhere("migrations")
        {
            let mut detected = DetectedRuntime::with_framework(
                RuntimeKind::Rust,
                "rust",
                vec![
                    "Cargo.toml:[workspace]".to_string(),
                    "migrations/".to_string(),
                ],
            );
            detected.migrate_command = if manifest.contains("diesel") {
                vec![
                    "diesel".to_string(),
                    "migration".to_string(),
                    "run".to_string(),
                ]
            } else {
                vec!["sqlx".to_string(), "migrate".to_string(), "run".to_string()]
            };
            detected.test_command = vec!["cargo".to_string(), "test".to_string()];
            return Some(detected);
        }
        let mut evidence = vec!["Cargo.toml".to_string()];
        let tier = if manifest.contains("axum")
            || manifest.contains("actix-web")
            || manifest.contains("rocket")
        {
            evidence.push("Cargo.toml:axum|actix-web|rocket".to_string());
            SupportTier::Tier2
        } else {
            SupportTier::Tier1
        };
        Some(DetectedRuntime::new(RuntimeKind::Rust, tier, evidence))
    }

    fn dev_command(&self) -> Vec<String> {
        vec!["cargo".to_string(), "run".to_string()]
    }

    fn prod_command(&self) -> Vec<String> {
        vec![
            "cargo".to_string(),
            "build".to_string(),
            "--release".to_string(),
        ]
    }

    fn prod_oci_image(&self) -> Option<String> {
        Some("oci://rust-app:prod".to_string())
    }

    fn health_path(&self) -> Option<String> {
        Some("/healthz".to_string())
    }
}

/// Expo adapter (mobile preview boundary; dev tunnel, exported bundle prod).
pub struct ExpoAdapter;

impl RuntimeAdapter for ExpoAdapter {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Expo
    }

    fn tier(&self) -> SupportTier {
        SupportTier::Tier1
    }

    fn detect(&self, snapshot: &ProjectSnapshot) -> Option<DetectedRuntime> {
        let package = snapshot.get("package.json")?;
        if !package.contains("expo") || !snapshot.has_file("app.json") {
            return None;
        }
        // Tier 3 Expo: file-based routing (`app/` dir) or build profiles
        // (`eas.json`) beyond the bare managed-workflow markers.
        if snapshot.has_dir_anywhere("app") || snapshot.has_file("eas.json") {
            let mut evidence = vec!["package.json:expo".to_string(), "app.json".to_string()];
            if snapshot.has_dir_anywhere("app") {
                evidence.push("app/".to_string());
            }
            if snapshot.has_file("eas.json") {
                evidence.push("eas.json".to_string());
            }
            let mut detected = DetectedRuntime::with_framework(RuntimeKind::Expo, "expo", evidence);
            if package.contains("jest") {
                detected.test_command = vec!["jest".to_string()];
            }
            return Some(detected);
        }
        Some(DetectedRuntime::new(
            RuntimeKind::Expo,
            SupportTier::Tier1,
            vec!["package.json:expo".to_string(), "app.json".to_string()],
        ))
    }

    fn dev_command(&self) -> Vec<String> {
        vec!["expo".to_string(), "start".to_string()]
    }

    fn prod_command(&self) -> Vec<String> {
        vec!["expo".to_string(), "export".to_string()]
    }

    fn health_path(&self) -> Option<String> {
        None
    }
}

/// Detects the richest applicable runtime for a snapshot.
///
/// Native adapters are tried before the generic container fallback, so a
/// Haskell project with only a Dockerfile lands on Tier 0 generic instead of
/// being rejected by language. Errors only when nothing matches.
pub fn detect_runtime(snapshot: &ProjectSnapshot) -> Result<DetectedRuntime> {
    if let Some(found) = ExpoAdapter.detect(snapshot) {
        return Ok(found);
    }
    if let Some(found) = NodeAdapter.detect(snapshot) {
        return Ok(found);
    }
    if let Some(found) = DotNetAdapter.detect(snapshot) {
        return Ok(found);
    }
    if let Some(found) = PythonAdapter.detect(snapshot) {
        return Ok(found);
    }
    if let Some(found) = RustAdapter.detect(snapshot) {
        return Ok(found);
    }
    if let Some(found) = GenericAdapter.detect(snapshot) {
        return Ok(found);
    }
    Err(CoreError::UnsupportedRuntime(
        "no recognized manifest or Dockerfile; runtime unsupported".to_string(),
    ))
}

/// Plans a [`RuntimeConfig`] for a detected runtime and profile.
///
/// Development never produces an OCI image; production always does (the OCI
/// production interchange boundary). A dev server command is never reused as
/// the production command.
pub fn prepare(
    detected: &DetectedRuntime,
    profile: RuntimeProfile,
    port: u16,
    healthcheck_path: Option<String>,
    dockerfile: Option<String>,
    limits: SandboxLimits,
) -> Result<RuntimeConfig> {
    if port == 0 {
        return Err(CoreError::InvalidRuntime(
            "port must be in 1..=65535".to_string(),
        ));
    }
    limits.validate()?;
    // Tier 3 detections carry deep framework conventions: dev/prod
    // commands and health defaults come from the resolved layout while
    // the OCI interchange and dev/prod invariants below still hold.
    let conventions = detected.conventions();
    let (run_command, oci_image, health_path, tier) = match detected.kind {
        RuntimeKind::Generic => (
            GenericAdapter.dev_command(),
            GenericAdapter.prod_oci_image(),
            None,
            SupportTier::Tier0,
        ),
        RuntimeKind::Node => (
            NodeAdapter.dev_command(),
            NodeAdapter.prod_oci_image(),
            NodeAdapter.health_path(),
            detected.tier,
        ),
        RuntimeKind::DotNet => (
            DotNetAdapter.dev_command(),
            DotNetAdapter.prod_oci_image(),
            DotNetAdapter.health_path(),
            detected.tier,
        ),
        RuntimeKind::Python => (
            conventions
                .as_ref()
                .map(|c| c.dev_command.clone())
                .unwrap_or_else(|| PythonAdapter.dev_command()),
            PythonAdapter.prod_oci_image(),
            conventions
                .as_ref()
                .and_then(|c| c.health_path.clone())
                .or_else(|| {
                    // Django defines no framework health endpoint: TCP-only.
                    if detected.framework.as_deref() == Some("django") {
                        None
                    } else {
                        PythonAdapter.health_path()
                    }
                }),
            detected.tier,
        ),
        RuntimeKind::Rust => (
            conventions
                .as_ref()
                .map(|c| c.dev_command.clone())
                .unwrap_or_else(|| RustAdapter.dev_command()),
            RustAdapter.prod_oci_image(),
            conventions
                .as_ref()
                .and_then(|c| c.health_path.clone())
                .or_else(|| RustAdapter.health_path()),
            detected.tier,
        ),
        RuntimeKind::Expo => (
            conventions
                .as_ref()
                .map(|c| c.dev_command.clone())
                .unwrap_or_else(|| ExpoAdapter.dev_command()),
            None,
            ExpoAdapter.health_path(),
            detected.tier,
        ),
    };
    let (run_command, oci_image) = match profile {
        RuntimeProfile::Development => (run_command, None),
        RuntimeProfile::Production => {
            let prod_command = match detected.kind {
                RuntimeKind::Generic => GenericAdapter.prod_command(),
                RuntimeKind::Node => NodeAdapter.prod_command(),
                RuntimeKind::DotNet => DotNetAdapter.prod_command(),
                RuntimeKind::Python => conventions
                    .as_ref()
                    .map(|c| c.prod_command.clone())
                    .unwrap_or_else(|| PythonAdapter.prod_command()),
                RuntimeKind::Rust => conventions
                    .as_ref()
                    .map(|c| c.prod_command.clone())
                    .unwrap_or_else(|| RustAdapter.prod_command()),
                RuntimeKind::Expo => conventions
                    .as_ref()
                    .map(|c| c.prod_command.clone())
                    .unwrap_or_else(|| ExpoAdapter.prod_command()),
            };
            (prod_command, oci_image)
        }
    };
    Ok(RuntimeConfig {
        kind: detected.kind,
        tier,
        profile,
        port,
        healthcheck_path: healthcheck_path.or(health_path),
        dockerfile: dockerfile.or_else(|| {
            if detected.kind == RuntimeKind::Generic {
                detected.evidence.first().cloned()
            } else {
                None
            }
        }),
        run_command,
        oci_image,
        limits,
    })
}

/// Simulates a build under sandbox limits (deterministic, no I/O).
///
/// Exceeding `timeout_secs` cancels and marks the build failed, carrying the
/// enforced limit and recovery detail. Finishing in time succeeds.
pub fn simulate_build(config: &RuntimeConfig, elapsed_secs: u64) -> BuildResult {
    if elapsed_secs > config.limits.timeout_secs {
        BuildResult::timed_out(config.limits.timeout_secs, elapsed_secs)
    } else {
        BuildResult::succeeded(format!(
            "{} build finished in {elapsed_secs}s on port {}",
            config.kind.canonical_name(),
            config.port
        ))
    }
}

/// Enforces process-count and memory caps for a (simulated) run.
///
/// Breaches cancel the run with the enforced limit named, so callers can
/// surface it in events and logs.
pub fn enforce_run_usage(config: &RuntimeConfig, processes: u32, memory_mb: u32) -> Result<()> {
    if processes > config.limits.max_processes {
        return Err(CoreError::LimitExceeded {
            limit: format!("max_processes={}", config.limits.max_processes),
            detail: format!(
                "process count {processes} exceeds sandbox limit {}",
                config.limits.max_processes
            ),
        });
    }
    if memory_mb > config.limits.memory_mb {
        return Err(CoreError::LimitExceeded {
            limit: format!("memory_mb={}", config.limits.memory_mb),
            detail: format!(
                "memory usage {memory_mb}MiB exceeds sandbox limit {}MiB",
                config.limits.memory_mb
            ),
        });
    }
    Ok(())
}

/// Evaluates health semantics for a profile.
///
/// Development accepts a dev-server 2xx/3xx on `/` (or TCP success when no
/// health path exists). Production requires the configured health path to
/// answer 2xx; a dev-server-shaped 3xx redirect or a 5xx in either profile is
/// unhealthy. This keeps "dev server up" from masquerading as "production
/// healthy".
pub fn evaluate_health(
    config: &RuntimeConfig,
    http_status: Option<u16>,
    path: Option<&str>,
) -> HealthStatus {
    match http_status {
        None => {
            if config.healthcheck_path.is_none() {
                HealthStatus::Unknown {
                    reason: "no health signal reported".to_string(),
                }
            } else {
                HealthStatus::Unhealthy {
                    reason: "no health signal reported".to_string(),
                }
            }
        }
        Some(status) => match config.profile {
            RuntimeProfile::Development => {
                if (200..400).contains(&status) {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy {
                        reason: format!("dev server answered {status}"),
                    }
                }
            }
            RuntimeProfile::Production => {
                let expected = config.healthcheck_path.as_deref();
                let path_ok = match (expected, path) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(e), Some(p)) => e == p,
                };
                if (200..300).contains(&status) && path_ok {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Unhealthy {
                        reason: format!(
                            "production health check failed: status {status} on path {} (expected {})",
                            path.unwrap_or("<none>"),
                            expected.unwrap_or("<tcp>"),
                        ),
                    }
                }
            }
        },
    }
}
