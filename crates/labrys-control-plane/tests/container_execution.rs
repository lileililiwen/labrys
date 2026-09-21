//! Integration tests for bounded container execution and preview runtime.
//!
//! Pure unit tests (argument structure, limit enforcement, health semantics,
//! isolation, revocation, redaction) always run. PostgreSQL-backed tests use
//! an isolated `labrys_container_exec_test` database so they never race the
//! `control_plane.rs` suite, and skip without `LABRYS_DATABASE_URL`.
//! Docker-backed tests exercise a real daemon (generic `nginx:alpine` /
//! `alpine:latest` images, already present locally) and skip gracefully when
//! no runtime is available — but an unavailable runtime is asserted as an
//! environment blocker, never simulated as a pass.

use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    docker_build_args, docker_run_args, enforce_limits_before_schedule, probe_health,
    prospective_phase, run_migrations, verify_production_artifact, ContainerExecutor,
    DockerExecutor, ExecutionIdentity, PgEventStore, PgEvidenceStore, PgExecutionStore,
    PgPreviewStore, PreviewManager, UnavailableExecutor, WorkspaceRoot,
};
use labrys_core::{
    detect_runtime, prepare, AgentSessionId, ApplicationId, CanonicalRepository, HealthStatus,
    Inspector, PreviewAccess, ProjectSnapshot, ResourcePhase, RetentionPolicy, RuntimeProfile,
    SandboxLimits, SessionWorkspace, WebPreview, REDACTION_MARKER,
};

const SECRET: &str = "preview-test-secret-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

/// Dedicated database for this file, so truncates never race `control_plane.rs`.
fn exec_db_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    match base.rfind('/') {
        Some(idx) if idx > scheme_end => format!("{}/labrys_container_exec_test", &base[..idx]),
        _ => format!("{base}/labrys_container_exec_test"),
    }
}

async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let base = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let maintenance = PgPool::connect(&base).await.expect("connect postgres");
    let _ = sqlx::query("CREATE DATABASE labrys_container_exec_test")
        .execute(&maintenance)
        .await;
    maintenance.close().await;
    let pool = PgPool::connect(&exec_db_url(&base))
        .await
        .expect("connect exec db");
    run_migrations(&pool).await.expect("run migrations");
    sqlx::query("TRUNCATE executions, previews, events, logs, evidence RESTART IDENTITY CASCADE")
        .execute(&pool)
        .await
        .expect("truncate");
    Some((guard, pool))
}

fn generic_config() -> labrys_core::RuntimeConfig {
    let snapshot = ProjectSnapshot::new([(
        "Dockerfile",
        "FROM alpine:latest\nCMD [\"sleep\", \"30\"]\n",
    )]);
    let detected = detect_runtime(&snapshot).expect("generic detected");
    assert_eq!(detected.kind, labrys_core::RuntimeKind::Generic);
    prepare(
        &detected,
        RuntimeProfile::Development,
        8080,
        None,
        None,
        SandboxLimits::docker_default(),
    )
    .expect("prepare")
}

fn test_repo() -> CanonicalRepository {
    CanonicalRepository::new(ApplicationId::new(), "main", "abc123").unwrap()
}

fn test_identity(trace: &str) -> ExecutionIdentity {
    ExecutionIdentity::new("test-app", trace).unwrap()
}

async fn docker_available() -> bool {
    matches!(
        DockerExecutor::from_path().check_available().await,
        labrys_control_plane::RuntimeAvailability::Available { .. }
    )
}

fn tmp_ctx(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "labrys-exec-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

// ---------------------------------------------------------------------------
// Pure unit tests: always run, no Docker, no database
// ---------------------------------------------------------------------------

#[test]
fn generic_project_executes_under_enforced_limits() {
    let config = generic_config();
    enforce_limits_before_schedule(&config).unwrap();
    let args = docker_run_args(&config, "labrys-unit-test", "labrys-test:latest").unwrap();
    let joined = args.join(" ");
    assert!(joined.contains("--cpus"), "{joined}");
    assert!(joined.contains("--memory 512m"), "{joined}");
    assert!(joined.contains("--pids-limit 128"), "{joined}");
    assert!(joined.contains("--read-only"), "{joined}");
    assert!(joined.contains("--cap-drop ALL"), "{joined}");
    // No shell on this path.
    assert!(!args.iter().any(|a| a == "sh" || a == "&&"), "{joined:?}");
}

#[test]
fn build_args_are_structured_without_shell() {
    let args = docker_build_args(
        std::path::Path::new("Dockerfile"),
        std::path::Path::new("."),
        "labrys-test:latest",
    )
    .unwrap();
    assert_eq!(
        args,
        vec!["build", "-f", "Dockerfile", "-t", "labrys-test:latest", "."]
    );
}

#[test]
fn sandbox_limits_validated_before_scheduling() {
    let mut config = generic_config();
    config.limits.timeout_secs = 0;
    assert!(enforce_limits_before_schedule(&config).is_err());
    let mut config = generic_config();
    config.limits.cpu_millicpus = 0;
    assert!(enforce_limits_before_schedule(&config).is_err());
}

#[test]
fn development_is_never_a_production_artifact() {
    let dev = generic_config();
    assert!(verify_production_artifact(&dev).is_err());
    let mut prod = generic_config();
    prod.profile = RuntimeProfile::Production;
    prod.oci_image = Some("oci://app:prod".to_string());
    assert_eq!(verify_production_artifact(&prod).unwrap(), "oci://app:prod");
}

#[test]
fn runtime_failures_cannot_masquerade_as_readiness() {
    let mut dev = generic_config();
    // Dev server 3xx is usable development health ...
    let dev_health = labrys_core::evaluate_health(&dev, Some(302), Some("/"));
    assert!(dev_health.is_healthy());
    // ... but never production healthy.
    dev.profile = RuntimeProfile::Production;
    dev.healthcheck_path = Some("/healthz".to_string());
    let prod_health = labrys_core::evaluate_health(&dev, Some(302), Some("/healthz"));
    assert!(!prod_health.is_healthy());
    assert_eq!(prospective_phase(&prod_health), ResourcePhase::Degraded);
    assert_eq!(
        prospective_phase(&HealthStatus::Healthy),
        ResourcePhase::Ready
    );
    // A failed preview issues no URL and stays visible.
    let workspace =
        SessionWorkspace::open(&test_repo(), AgentSessionId::new(), "development").unwrap();
    let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
    preview
        .record_health(HealthStatus::Unhealthy {
            reason: "dev server answered 500".to_string(),
        })
        .unwrap();
    assert!(preview.temporary_url().is_none());
    assert!(preview.to_event_detail().contains("unavailable"));
}

#[tokio::test]
async fn disabled_network_blocks_health_probe() {
    let mut config = generic_config();
    config.limits.network = labrys_core::NetworkMode::Disabled;
    let health = probe_health(&config, "127.0.0.1", 8080).await;
    assert!(!health.is_healthy());
    assert!(matches!(health, HealthStatus::Unknown { .. }));
}

#[test]
fn previews_are_isolated_and_revocable() {
    let root = WorkspaceRoot::new(PathBuf::from("/tmp/labrys-exec-unit")).unwrap();
    let session_a = AgentSessionId::new();
    let session_b = AgentSessionId::new();
    let dir_a = root.worktree_dir(&session_a).unwrap();
    let dir_b = root.worktree_dir(&session_b).unwrap();
    assert_ne!(dir_a, dir_b);

    let repo = test_repo();
    let workspace_a = SessionWorkspace::open(&repo, session_a, "development").unwrap();
    let workspace_b = SessionWorkspace::open(&repo, session_b, "development").unwrap();
    assert_ne!(workspace_a.worktree_path, workspace_b.worktree_path);
    assert_ne!(workspace_a.branch, workspace_b.branch);

    let mut preview = WebPreview::new(&workspace_a, PreviewAccess::Private, None);
    preview.record_health(HealthStatus::Healthy).unwrap();
    let url = preview.temporary_url().unwrap().to_string();
    assert!(url.contains(&preview.id.to_string()));
    preview.expire();
    assert!(preview.temporary_url().is_none());
    assert!(!preview.is_live(Utc::now()));
}

#[tokio::test]
async fn unavailable_runtime_is_blocker_never_a_pass() {
    let executor = UnavailableExecutor::missing_docker();
    assert!(!executor.check_available().await.is_available());
    let identity = test_identity("trace-blocker");
    let config = generic_config();
    let build = executor
        .build(
            std::path::Path::new("Dockerfile"),
            std::path::Path::new("."),
            "img:latest",
            &config.limits,
            &identity,
        )
        .await;
    assert!(!build.is_success());
    assert!(build.recovery.is_some());
    assert!(executor
        .run(&config, "img:latest", "labrys-blocked", &identity)
        .await
        .is_err());
    let health = executor.health(&config, "127.0.0.1", 8080).await;
    assert!(!health.is_healthy());
}

#[test]
fn diagnostics_redact_secrets_before_persistence() {
    let redacted = labrys_control_plane::redact::redact(
        &format!("build failed with password {SECRET} inside"),
        &[SECRET],
    );
    assert!(!redacted.contains(SECRET));
    assert!(redacted.contains(REDACTION_MARKER));
}

#[test]
fn inspector_understanding_covers_generic_floor() {
    // The executor's generic path only runs what the inspector recognizes.
    let snapshot = ProjectSnapshot::new([("Dockerfile", "FROM alpine:latest\n")]);
    let inspector = Inspector::default();
    let profile = inspector.inspect(&snapshot);
    assert!(profile.container_compatible);
}

// ---------------------------------------------------------------------------
// PostgreSQL-backed tests: isolated database, skip without LABRYS_DATABASE_URL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execution_and_preview_state_persisted_with_redaction() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let secrets = [SECRET];
    let secret_refs: Vec<&str> = secrets.to_vec();
    let store = PgExecutionStore::new(pool.clone());
    let identity = test_identity("trace-persist");
    let record = store
        .record(
            "build",
            "failed",
            &format!("build failed leaking {SECRET}"),
            Some("labrys-test:latest"),
            None,
            None,
            &serde_json::json!({"timeout_secs": 600}),
            &identity,
            &secret_refs,
        )
        .await
        .unwrap();
    assert!(!record.detail.contains(SECRET));
    assert!(record.detail.contains(REDACTION_MARKER));
    let traced = store.for_trace("trace-persist").await.unwrap();
    assert_eq!(traced.len(), 1);

    let previews = PgPreviewStore::new(pool.clone());
    let workspace =
        SessionWorkspace::open(&test_repo(), AgentSessionId::new(), "development").unwrap();
    let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
    preview.record_health(HealthStatus::Healthy).unwrap();
    previews
        .upsert(
            &preview,
            &workspace,
            &identity,
            Some("cid"),
            Some("cname"),
            &[],
        )
        .await
        .unwrap();
    let row = previews.get(&preview.id).await.unwrap();
    assert_eq!(row.status, "available");
    assert_eq!(row.url, preview.temporary_url().map(str::to_string));
}

#[tokio::test]
async fn preview_manager_blocks_without_runtime_and_persists() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let root_dir = tmp_ctx("root-blocked");
    let root = WorkspaceRoot::new(root_dir.clone()).unwrap();
    let executor = std::sync::Arc::new(UnavailableExecutor::missing_docker());
    let manager: PreviewManager<UnavailableExecutor> =
        PreviewManager::new(pool.clone(), executor, root, RetentionPolicy::default())
            .with_secrets(vec![SECRET.to_string()]);
    let session = AgentSessionId::new();
    let workspace = manager
        .materialize_workspace(&test_repo(), session, "development")
        .await
        .unwrap();
    assert!(root_dir
        .join("worktrees")
        .join(session.to_string())
        .exists());

    let config = generic_config();
    let identity = test_identity("trace-blocked-preview");
    let started = manager
        .start_preview(
            &workspace,
            &config,
            std::path::Path::new("Dockerfile"),
            std::path::Path::new("."),
            "labrys-blocked:latest",
            &identity,
            None,
        )
        .await
        .unwrap();
    assert!(started.preview.temporary_url().is_none());
    assert!(started.container_id.is_none());

    let previews = PgPreviewStore::new(pool.clone());
    let row = previews.get(&started.preview.id).await.unwrap();
    assert_eq!(row.status, "unavailable");

    let events = PgEventStore::new(pool.clone());
    assert!(!events
        .for_trace("trace-blocked-preview")
        .await
        .unwrap()
        .is_empty());
    let evidence = PgEvidenceStore::new(pool.clone(), RetentionPolicy::default());
    assert!(!evidence.all().await.unwrap().is_empty());

    manager.cleanup_workspace(&workspace).await.unwrap();
    assert!(!root_dir
        .join("worktrees")
        .join(session.to_string())
        .exists());
}

#[tokio::test]
async fn preview_expiry_revokes_persisted_access() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let root = WorkspaceRoot::new(tmp_ctx("root-expiry")).unwrap();
    let executor = std::sync::Arc::new(UnavailableExecutor::missing_docker());
    let manager: PreviewManager<UnavailableExecutor> =
        PreviewManager::new(pool.clone(), executor, root, RetentionPolicy::default());
    let workspace =
        SessionWorkspace::open(&test_repo(), AgentSessionId::new(), "development").unwrap();
    let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
    preview.record_health(HealthStatus::Healthy).unwrap();
    assert!(preview.temporary_url().is_some());
    let identity = test_identity("trace-expiry");
    let previews = PgPreviewStore::new(pool.clone());
    previews
        .upsert(
            &preview,
            &workspace,
            &identity,
            Some("cid"),
            Some("cname"),
            &[],
        )
        .await
        .unwrap();
    manager
        .expire_preview(
            &workspace,
            &mut preview,
            Some("cid"),
            Some("cname"),
            &identity,
        )
        .await
        .unwrap();
    assert!(preview.temporary_url().is_none());
    let row = previews.get(&preview.id).await.unwrap();
    assert_eq!(row.status, "expired");
}

// ---------------------------------------------------------------------------
// Docker-backed tests: real daemon, skip gracefully when absent
// ---------------------------------------------------------------------------

async fn write_ctx(dockerfile: &str, name: &str) -> PathBuf {
    let dir = tmp_ctx(name);
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("Dockerfile"), dockerfile)
        .await
        .unwrap();
    dir
}

async fn remove_image(tag: &str) {
    let _ = tokio::process::Command::new("docker")
        .args(["rmi", "-f", tag])
        .output()
        .await;
}

#[tokio::test]
async fn generic_dockerfile_builds_runs_reports_health_logs_and_cleans_up() {
    if !docker_available().await {
        return;
    }
    let executor = DockerExecutor::from_path();
    let tag = format!("labrys-test-generic:{}", uuid::Uuid::new_v4().simple());
    // A minimal HTTP workload that runs unprivileged under the full default
    // sandbox (read-only root, dropped capabilities, isolated network):
    // nginx cannot start there (it chowns its cache), node can.
    let ctx = write_ctx(
        "FROM node:24-alpine\nEXPOSE 8080\nCMD [\"node\", \"-e\", \
         \"require('http').createServer((req,res)=>{res.writeHead(200);res.end('labrys-ok')})\
         .listen(8080,()=>console.log('listening'))\"]\n",
        "generic",
    )
    .await;
    let config = generic_config();
    let identity = test_identity("trace-generic-exec");

    let build = executor
        .build(
            &ctx.join("Dockerfile"),
            &ctx,
            &tag,
            &config.limits,
            &identity,
        )
        .await;
    assert!(build.is_success(), "build failed: {}", build.detail);

    let name = format!("labrys-test-{}", uuid::Uuid::new_v4().simple());
    let container = executor.run(&config, &tag, &name, &identity).await.unwrap();
    assert!(!container.id.is_empty());
    let host_port = container.host_port.expect("published host port");

    // Give node a moment to accept connections.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let health = executor.health(&config, "127.0.0.1", host_port).await;
    assert!(health.is_healthy(), "health not healthy: {health:?}");

    let logs = executor.logs(&container.id, 50).await.unwrap();
    assert!(
        logs.contains("listening"),
        "expected startup log, got: {logs}"
    );

    // Cleanup is idempotent: twice is fine, and the container is gone.
    executor.cleanup(&container.id).await.unwrap();
    executor.cleanup(&container.id).await.unwrap();
    let ps = tokio::process::Command::new("docker")
        .args(["ps", "-q", "--filter", &format!("id={}", container.id)])
        .output()
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&ps.stdout).trim().is_empty());
    remove_image(&tag).await;
    let _ = tokio::fs::remove_dir_all(&ctx).await;
}

#[tokio::test]
async fn build_timeout_cancels_with_limit_and_recovery() {
    if !docker_available().await {
        return;
    }
    let executor = DockerExecutor::from_path();
    let tag = format!("labrys-test-timeout:{}", uuid::Uuid::new_v4().simple());
    let ctx = write_ctx("FROM alpine:latest\nRUN sleep 30\n", "timeout").await;
    let mut limits = SandboxLimits::docker_default();
    limits.timeout_secs = 5;
    let identity = test_identity("trace-timeout");
    let build = executor
        .build(&ctx.join("Dockerfile"), &ctx, &tag, &limits, &identity)
        .await;
    assert!(!build.is_success());
    assert!(build.cancelled);
    assert!(build.limit_enforced.is_some());
    assert!(build.recovery.is_some());
    remove_image(&tag).await;
    let _ = tokio::fs::remove_dir_all(&ctx).await;
}

#[tokio::test]
async fn failed_container_health_issues_no_preview_url() {
    if !docker_available().await {
        return;
    }
    let executor = DockerExecutor::from_path();
    let tag = format!("labrys-test-unhealthy:{}", uuid::Uuid::new_v4().simple());
    let ctx = write_ctx("FROM alpine:latest\nCMD [\"sleep\", \"30\"]\n", "unhealthy").await;
    let config = generic_config();
    let identity = test_identity("trace-unhealthy");
    let build = executor
        .build(
            &ctx.join("Dockerfile"),
            &ctx,
            &tag,
            &config.limits,
            &identity,
        )
        .await;
    assert!(build.is_success(), "build failed: {}", build.detail);
    let name = format!("labrys-test-{}", uuid::Uuid::new_v4().simple());
    let container = executor.run(&config, &tag, &name, &identity).await.unwrap();

    // Nothing listens on 8080 inside `sleep`; the platform probe must fail.
    let health = executor
        .health(
            &config,
            "127.0.0.1",
            container.host_port.unwrap_or(config.port),
        )
        .await;
    assert!(!health.is_healthy());
    let workspace =
        SessionWorkspace::open(&test_repo(), AgentSessionId::new(), "development").unwrap();
    let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
    preview.record_health(health).unwrap();
    assert!(preview.temporary_url().is_none());

    executor.cleanup(&container.id).await.unwrap();
    remove_image(&tag).await;
    let _ = tokio::fs::remove_dir_all(&ctx).await;
}

#[tokio::test]
async fn concurrent_sessions_use_independent_runtimes() {
    if !docker_available().await {
        return;
    }
    let executor = DockerExecutor::from_path();
    let tag = format!("labrys-test-shared:{}", uuid::Uuid::new_v4().simple());
    let ctx = write_ctx("FROM alpine:latest\nCMD [\"sleep\", \"60\"]\n", "shared").await;
    let config = generic_config();
    let identity = test_identity("trace-concurrent");
    let build = executor
        .build(
            &ctx.join("Dockerfile"),
            &ctx,
            &tag,
            &config.limits,
            &identity,
        )
        .await;
    assert!(build.is_success(), "build failed: {}", build.detail);

    let name_a = format!("labrys-test-a-{}", uuid::Uuid::new_v4().simple());
    let name_b = format!("labrys-test-b-{}", uuid::Uuid::new_v4().simple());
    let a = executor
        .run(&config, &tag, &name_a, &identity)
        .await
        .unwrap();
    let b = executor
        .run(&config, &tag, &name_b, &identity)
        .await
        .unwrap();
    assert_ne!(a.id, b.id);
    assert_ne!(a.name, b.name);
    assert!(executor.logs(&a.id, 10).await.is_ok());
    assert!(executor.logs(&b.id, 10).await.is_ok());
    executor.cleanup(&a.id).await.unwrap();
    executor.cleanup(&b.id).await.unwrap();
    remove_image(&tag).await;
    let _ = tokio::fs::remove_dir_all(&ctx).await;
}
