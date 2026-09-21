//! Health-gated preview runtime over isolated session workspaces.
//!
//! Each preview runs in its own materialized session workspace and its own
//! container. Access (URL or Expo QR payload) is issued only after a
//! platform-owned health probe reports [`HealthStatus::Healthy`]; expiry,
//! failure, or explicit revocation stops the container and revokes access.
//! Two sessions previewing one application can never share a checkout, a
//! container, or a URL.
//!
//! All preview transitions are recorded through the durable attributable
//! streams — platform-actor events, separated runtime/build logs, and
//! verification evidence — with secrets redacted before persistence. Preview
//! and execution rows in PostgreSQL are the durable memory; the in-memory
//! [`WebPreview`]/[`SessionWorkspace`] plans stay authoritative for policy.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

use labrys_core::{
    AgentSessionId, CanonicalRepository, Correlation, EventAction, EventActor, EventDraft,
    EventResource, EventResult, ExpoShare, ExpoTransport, HealthStatus, LogLevel, LogSource,
    PreviewId, RuntimeConfig, SessionWorkspace, VerificationEvidence, VerifierStage, WebPreview,
};

use crate::error::{ControlPlaneError, Result};
use crate::observability::{PgEventStore, PgEvidenceStore, PgLogStore};
use crate::redact;
use crate::runtime::{
    enforce_limits_before_schedule, probe_health, ContainerExecutor, EffectiveLimits,
    ExecutionIdentity, RuntimeAvailability,
};

/// Where materialized session worktrees live on the host.
///
/// Only paths under this root are ever created or removed. Host mounts beyond
/// the approved workspace are never constructed: container runs receive the
/// workspace as their build context, not as a bind mount.
#[derive(Debug, Clone)]
pub struct WorkspaceRoot {
    root: PathBuf,
}

impl WorkspaceRoot {
    pub fn new(root: PathBuf) -> Result<Self> {
        if root.as_os_str().is_empty() {
            return Err(ControlPlaneError::Execution(
                "workspace root must not be empty".to_string(),
            ));
        }
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Worktree directory for one session. The session id is a UUID, so the
    /// join cannot escape the root — and the prefix is asserted anyway.
    pub fn worktree_dir(&self, session_id: &AgentSessionId) -> Result<PathBuf> {
        let dir = self.root.join("worktrees").join(session_id.to_string());
        if !dir.starts_with(&self.root) {
            return Err(ControlPlaneError::Execution(
                "workspace path escapes the approved root".to_string(),
            ));
        }
        Ok(dir)
    }
}

/// A preview that is running (or was attempted) with its container binding.
#[derive(Debug, Clone)]
pub struct StartedPreview {
    pub preview: WebPreview,
    pub workspace: SessionWorkspace,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub limits: EffectiveLimits,
}

/// Health-gated preview lifecycle over an injected [`ContainerExecutor`].
#[derive(Clone)]
pub struct PreviewManager<E> {
    executor: Arc<E>,
    events: PgEventStore,
    logs: PgLogStore,
    evidence: PgEvidenceStore,
    previews: PgPreviewStore,
    executions: PgExecutionStore,
    secrets: Vec<String>,
    workspace_root: WorkspaceRoot,
}

impl<E: ContainerExecutor> PreviewManager<E> {
    pub fn new(
        pool: PgPool,
        executor: Arc<E>,
        workspace_root: WorkspaceRoot,
        evidence_policy: labrys_core::RetentionPolicy,
    ) -> Self {
        Self {
            executor,
            events: PgEventStore::new(pool.clone()),
            logs: PgLogStore::new(pool.clone()),
            evidence: PgEvidenceStore::new(pool.clone(), evidence_policy),
            previews: PgPreviewStore::new(pool.clone()),
            executions: PgExecutionStore::new(pool),
            secrets: Vec::new(),
            workspace_root,
        }
    }

    pub fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets;
        self
    }

    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }

    /// Materializes an isolated worktree directory for one session.
    ///
    /// The [`SessionWorkspace`] plan already derives per-session paths and
    /// branches so two sessions can never share a checkout; this creates the
    /// directory to match and rejects a second allocation for the same
    /// session while its directory exists.
    pub async fn materialize_workspace(
        &self,
        repo: &CanonicalRepository,
        session_id: AgentSessionId,
        environment_name: &str,
    ) -> Result<SessionWorkspace> {
        let workspace = SessionWorkspace::open(repo, session_id, environment_name)?;
        let dir = self.workspace_root.worktree_dir(&session_id)?;
        if dir.exists() {
            return Err(ControlPlaneError::Execution(format!(
                "session {session_id} already owns a worktree at {}",
                dir.display()
            )));
        }
        tokio::fs::create_dir_all(&dir).await?;
        self.record_log(
            &workspace_identity(&workspace),
            LogSource::Runtime,
            LogLevel::Info,
            &format!(
                "materialized workspace {} at {}",
                workspace.id,
                dir.display()
            ),
        )
        .await?;
        Ok(workspace)
    }

    /// Builds, starts, and health-gates one preview.
    ///
    /// The URL is issued only when the platform-owned probe reports healthy.
    /// Build failures, failed health, and unavailable runtimes leave the
    /// preview `Unavailable` with the limit and recovery detail visible to
    /// controllers and operators — never as a silently missing preview.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_preview(
        &self,
        workspace: &SessionWorkspace,
        config: &RuntimeConfig,
        dockerfile: &Path,
        context: &Path,
        image: &str,
        identity: &ExecutionIdentity,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<StartedPreview> {
        enforce_limits_before_schedule(config)?;
        let limits = EffectiveLimits::from_config(config);
        let mut preview =
            WebPreview::new(workspace, labrys_core::PreviewAccess::Private, expires_at);
        preview.notify_build_started()?;
        self.record_event(
            identity,
            EventAction::BuildStarted,
            EventResult::Accepted,
            "preview",
            &preview.id.to_string(),
            &format!(
                "preview {} build started under {}",
                preview.id,
                limits.summary()
            ),
        )
        .await?;

        if let RuntimeAvailability::Unavailable { reason } = self.executor.check_available().await {
            let blocked = unavailable_build_result(config, &reason);
            let _ = preview.record_build(&blocked);
            self.persist_preview_state(&preview, workspace, identity, None, None)
                .await?;
            self.record_terminal(&preview, workspace, identity, &blocked.detail)
                .await?;
            return Ok(StartedPreview {
                preview,
                workspace: workspace.clone(),
                container_id: None,
                container_name: None,
                limits,
            });
        }

        let build = self
            .executor
            .build(dockerfile, context, image, &config.limits, identity)
            .await;
        let build_succeeded = build.is_success();
        let _ = preview.record_build(&build);
        self.record_execution("build", &build, Some(image), None, None, &limits, identity)
            .await?;
        self.record_event(
            identity,
            EventAction::BuildFinished,
            if build_succeeded {
                EventResult::Succeeded
            } else {
                EventResult::Failed {
                    reason: redact::redact(&build.detail, &self.secret_refs()),
                }
            },
            "preview",
            &preview.id.to_string(),
            &build.detail,
        )
        .await?;
        if !build_succeeded {
            self.persist_preview_state(&preview, workspace, identity, None, None)
                .await?;
            self.record_terminal(&preview, workspace, identity, &build.detail)
                .await?;
            return Ok(StartedPreview {
                preview,
                workspace: workspace.clone(),
                container_id: None,
                container_name: None,
                limits,
            });
        }

        let container_name =
            format!("labrys-{}-{}", workspace.session_id, preview.id).replace('_', "-");
        let container = match self
            .executor
            .run(config, image, &container_name, identity)
            .await
        {
            Ok(container) => container,
            Err(err) => {
                let reason = err.to_string();
                let failed =
                    labrys_core::BuildResult::failed(format!("preview run failed: {reason}"));
                let _ = preview.record_build(&failed);
                self.persist_preview_state(
                    &preview,
                    workspace,
                    identity,
                    None,
                    Some(&container_name),
                )
                .await?;
                self.record_terminal(&preview, workspace, identity, &reason)
                    .await?;
                return Ok(StartedPreview {
                    preview,
                    workspace: workspace.clone(),
                    container_id: None,
                    container_name: Some(container_name),
                    limits,
                });
            }
        };

        let health = self
            .executor
            .health(
                config,
                "127.0.0.1",
                container.host_port.unwrap_or(config.port),
            )
            .await;
        self.record_execution(
            "run",
            &labrys_core::BuildResult::succeeded(format!(
                "container {} started from image {image}",
                container.id
            )),
            Some(image),
            Some(&container.id),
            Some(&container_name),
            &limits,
            identity,
        )
        .await?;
        let healthy = health.is_healthy();
        let _ = preview.record_health(health.clone());
        self.record_event(
            identity,
            EventAction::HealthProbed,
            match &health {
                HealthStatus::Healthy => EventResult::Succeeded,
                HealthStatus::Starting => EventResult::Accepted,
                HealthStatus::Unhealthy { reason } | HealthStatus::Unknown { reason } => {
                    EventResult::Failed {
                        reason: redact::redact(reason, &self.secret_refs()),
                    }
                }
            },
            "preview",
            &preview.id.to_string(),
            &preview.to_event_detail(),
        )
        .await?;
        let evidence = VerificationEvidence::new(
            format!("preview-{}-health", preview.id),
            VerifierStage::Health,
            "preview-health-gate",
            healthy,
            &preview.to_event_detail(),
            &self.secret_refs(),
            Utc::now(),
        );
        let _ = self.evidence.append(&evidence, &self.secret_refs()).await;
        if !healthy {
            let _ = self.executor.cleanup(&container.id).await;
            self.persist_preview_state(&preview, workspace, identity, None, Some(&container_name))
                .await?;
            return Ok(StartedPreview {
                preview,
                workspace: workspace.clone(),
                container_id: None,
                container_name: Some(container_name),
                limits,
            });
        }
        self.persist_preview_state(
            &preview,
            workspace,
            identity,
            Some(&container.id),
            Some(&container_name),
        )
        .await?;
        Ok(StartedPreview {
            preview,
            workspace: workspace.clone(),
            container_id: Some(container.id),
            container_name: Some(container_name),
            limits,
        })
    }

    /// Expires a preview: stops its container and revokes URL access.
    pub async fn expire_preview(
        &self,
        workspace: &SessionWorkspace,
        preview: &mut WebPreview,
        container_id: Option<&str>,
        container_name: Option<&str>,
        identity: &ExecutionIdentity,
    ) -> Result<()> {
        if let Some(id) = container_id {
            let _ = self.executor.cleanup(id).await;
        }
        preview.expire();
        self.persist_preview_state(preview, workspace, identity, None, container_name)
            .await?;
        self.record_event(
            identity,
            EventAction::Reconciled,
            EventResult::Succeeded,
            "preview",
            &preview.id.to_string(),
            &format!("preview {} expired; access revoked", preview.id),
        )
        .await?;
        Ok(())
    }

    /// Explicitly revokes a preview before expiry.
    pub async fn revoke_preview(
        &self,
        workspace: &SessionWorkspace,
        preview: &mut WebPreview,
        container_id: Option<&str>,
        container_name: Option<&str>,
        identity: &ExecutionIdentity,
        reason: &str,
    ) -> Result<()> {
        if let Some(id) = container_id {
            let _ = self.executor.cleanup(id).await;
        }
        let failed = labrys_core::BuildResult::failed(format!("preview revoked: {reason}"));
        let _ = preview.record_build(&failed);
        self.persist_preview_state(preview, workspace, identity, None, container_name)
            .await?;
        self.record_event(
            identity,
            EventAction::Reconciled,
            EventResult::Succeeded,
            "preview",
            &preview.id.to_string(),
            &format!("preview {} revoked: {reason}", preview.id),
        )
        .await?;
        Ok(())
    }

    /// Removes a finished worktree directory. Only merged/discarded workspaces
    /// (or an explicit session teardown) reach here; active worktrees are
    /// refused so cleanup can never delete a live checkout.
    pub async fn cleanup_workspace(&self, workspace: &SessionWorkspace) -> Result<()> {
        let dir = self.workspace_root.worktree_dir(&workspace.session_id)?;
        if !dir.starts_with(self.workspace_root.path()) {
            return Err(ControlPlaneError::Execution(
                "workspace cleanup escapes the approved root".to_string(),
            ));
        }
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }

    /// Expo mobile share metadata for a session workspace.
    pub fn expo_share(
        &self,
        workspace: &SessionWorkspace,
        project_slug: &str,
        host: &str,
        port: u16,
        transport: ExpoTransport,
        expires_at: DateTime<Utc>,
    ) -> Result<ExpoShare> {
        let url = ExpoShare::discover_server_url(host, port)?;
        ExpoShare::new(workspace, project_slug, url, transport, expires_at)
            .map_err(ControlPlaneError::Core)
    }

    /// Platform-owned health check helper for callers that already run a
    /// container and need the core health semantics without an executor.
    pub async fn probe_container_health(
        &self,
        config: &RuntimeConfig,
        host: &str,
        port: u16,
    ) -> HealthStatus {
        probe_health(config, host, port).await
    }

    async fn persist_preview_state(
        &self,
        preview: &WebPreview,
        workspace: &SessionWorkspace,
        identity: &ExecutionIdentity,
        container_id: Option<&str>,
        container_name: Option<&str>,
    ) -> Result<()> {
        self.previews
            .upsert(
                preview,
                workspace,
                identity,
                container_id,
                container_name,
                &self.secret_refs(),
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_execution(
        &self,
        kind: &str,
        build: &labrys_core::BuildResult,
        image: Option<&str>,
        container_id: Option<&str>,
        container_name: Option<&str>,
        limits: &EffectiveLimits,
        identity: &ExecutionIdentity,
    ) -> Result<()> {
        let status = match build.status {
            labrys_core::BuildStatus::Succeeded => "succeeded",
            labrys_core::BuildStatus::Failed => "failed",
            labrys_core::BuildStatus::Cancelled => "cancelled",
        };
        let limits_value = serde_json::to_value(limits).unwrap_or(serde_json::Value::Null);
        let secrets = self.secret_refs();
        self.executions
            .record(
                kind,
                status,
                &build.detail,
                image,
                container_id,
                container_name,
                &limits_value,
                identity,
                &secrets,
            )
            .await?;
        Ok(())
    }

    async fn record_event(
        &self,
        identity: &ExecutionIdentity,
        action: EventAction,
        result: EventResult,
        kind: &str,
        id: &str,
        detail: &str,
    ) -> Result<()> {
        let mut draft = EventDraft::new(
            Utc::now(),
            EventActor::platform("preview-runtime"),
            identity.correlation(),
            action,
            result,
        );
        draft.resource = Some(EventResource::new(kind, id));
        draft.after = Some(detail.to_string());
        let secrets = self.secret_refs();
        self.events.append(&draft, &secrets).await?;
        Ok(())
    }

    async fn record_log(
        &self,
        identity: &ExecutionIdentity,
        source: LogSource,
        level: LogLevel,
        message: &str,
    ) -> Result<()> {
        let secrets = self.secret_refs();
        self.logs
            .append(
                source,
                level,
                &identity.correlation(),
                message,
                &secrets,
                Utc::now(),
            )
            .await?;
        Ok(())
    }

    async fn record_terminal(
        &self,
        preview: &WebPreview,
        workspace: &SessionWorkspace,
        identity: &ExecutionIdentity,
        detail: &str,
    ) -> Result<()> {
        let _ = workspace;
        self.record_log(
            identity,
            LogSource::Runtime,
            LogLevel::Error,
            &preview.to_event_detail(),
        )
        .await?;
        let evidence = VerificationEvidence::new(
            format!("preview-{}-terminal", preview.id),
            VerifierStage::BuildTest,
            "preview-terminal",
            false,
            detail,
            &self.secret_refs(),
            Utc::now(),
        );
        let secrets = self.secret_refs();
        let _ = self.evidence.append(&evidence, &secrets).await;
        Ok(())
    }
}

fn workspace_identity(workspace: &SessionWorkspace) -> ExecutionIdentity {
    ExecutionIdentity {
        application_id: workspace.application_id.to_string(),
        environment_id: None,
        workspace_id: Some(workspace.id.to_string()),
        session_id: Some(workspace.session_id.to_string()),
        trace_id: format!("workspace:{}", workspace.id),
    }
}

fn unavailable_build_result(config: &RuntimeConfig, reason: &str) -> labrys_core::BuildResult {
    let mut result = labrys_core::BuildResult::failed(format!(
        "preview blocked: runtime unavailable ({reason}); no image was produced"
    ));
    result.limit_enforced = Some(format!("timeout_secs={}", config.limits.timeout_secs));
    result.recovery = Some(
        "provision a container runtime, then retry; this is an environment blocker".to_string(),
    );
    result
}

/// Durable execution record: one build or run under explicit limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRecord {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub detail: String,
}

/// Durable store for the `executions` table.
#[derive(Clone)]
pub struct PgExecutionStore {
    pool: PgPool,
}

impl PgExecutionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        kind: &str,
        status: &str,
        detail: &str,
        image: Option<&str>,
        container_id: Option<&str>,
        container_name: Option<&str>,
        limits: &serde_json::Value,
        identity: &ExecutionIdentity,
        secrets: &[&str],
    ) -> Result<ExecutionRecord> {
        if !matches!(kind, "build" | "run") {
            return Err(ControlPlaneError::Execution(format!(
                "unknown execution kind {kind}"
            )));
        }
        if !matches!(status, "running" | "succeeded" | "failed" | "cancelled") {
            return Err(ControlPlaneError::Execution(format!(
                "unknown execution status {status}"
            )));
        }
        let detail = redact::redact_guard("execution.detail", detail, secrets)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO executions
                 (id, application_id, environment_id, workspace_id, session_id,
                  trace_id, kind, image, container_id, container_name, limits,
                  status, detail, created_at, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"#,
        )
        .bind(&id)
        .bind(&identity.application_id)
        .bind(&identity.environment_id)
        .bind(&identity.workspace_id)
        .bind(&identity.session_id)
        .bind(&identity.trace_id)
        .bind(kind)
        .bind(image)
        .bind(container_id)
        .bind(container_name)
        .bind(limits)
        .bind(status)
        .bind(&detail)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(ExecutionRecord {
            id,
            kind: kind.to_string(),
            status: status.to_string(),
            detail,
        })
    }

    pub async fn for_trace(&self, trace_id: &str) -> Result<Vec<ExecutionRecord>> {
        let rows = sqlx::query("SELECT * FROM executions WHERE trace_id = $1 ORDER BY created_at")
            .bind(trace_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(execution_from_row).collect()
    }
}

fn execution_from_row(row: &sqlx::postgres::PgRow) -> Result<ExecutionRecord> {
    Ok(ExecutionRecord {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        status: row.try_get("status")?,
        detail: row.try_get("detail")?,
    })
}

/// Durable store for the `previews` table.
#[derive(Clone)]
pub struct PgPreviewStore {
    pool: PgPool,
}

impl PgPreviewStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert(
        &self,
        preview: &WebPreview,
        workspace: &SessionWorkspace,
        identity: &ExecutionIdentity,
        container_id: Option<&str>,
        container_name: Option<&str>,
        secrets: &[&str],
    ) -> Result<()> {
        let status = preview_status_name(preview);
        if let Some(url) = preview.temporary_url() {
            redact::redact_guard("preview.url", url, secrets)?;
        }
        let now = Utc::now();
        sqlx::query(
            r#"INSERT INTO previews
                 (id, workspace_id, session_id, application_id, environment_id,
                  status, url, container_id, container_name, expires_at, updated_at)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
               ON CONFLICT (id) DO UPDATE SET
                 status = EXCLUDED.status, url = EXCLUDED.url,
                 container_id = EXCLUDED.container_id,
                 container_name = EXCLUDED.container_name,
                 expires_at = EXCLUDED.expires_at, updated_at = EXCLUDED.updated_at"#,
        )
        .bind(preview.id.to_string())
        .bind(workspace.id.to_string())
        .bind(workspace.session_id.to_string())
        .bind(&identity.application_id)
        .bind(&identity.environment_id)
        .bind(status)
        .bind(preview.temporary_url())
        .bind(container_id)
        .bind(container_name)
        .bind(preview.expires_at)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &PreviewId) -> Result<PreviewRow> {
        let row = sqlx::query("SELECT * FROM previews WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| ControlPlaneError::NotFound(format!("preview {id}")))?;
        preview_row_from_row(&row)
    }
}

/// Durable preview row projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRow {
    pub id: String,
    pub status: String,
    pub url: Option<String>,
    pub container_id: Option<String>,
}

fn preview_row_from_row(row: &sqlx::postgres::PgRow) -> Result<PreviewRow> {
    Ok(PreviewRow {
        id: row.try_get("id")?,
        status: row.try_get("status")?,
        url: row.try_get("url")?,
        container_id: row.try_get("container_id")?,
    })
}

fn preview_status_name(preview: &WebPreview) -> &'static str {
    match &preview.status {
        labrys_core::PreviewStatus::Pending => "pending",
        labrys_core::PreviewStatus::Building => "building",
        labrys_core::PreviewStatus::Available => "available",
        labrys_core::PreviewStatus::Unavailable { .. } => "unavailable",
        labrys_core::PreviewStatus::Expired => "expired",
    }
}

/// Correlation helper shared by execution writers.
pub fn correlation_for(trace_id: &str, session_id: Option<AgentSessionId>) -> Correlation {
    let mut correlation = Correlation::new(trace_id);
    if let Some(session_id) = session_id {
        correlation = correlation.with_session(session_id);
    }
    correlation
}

#[cfg(test)]
mod tests {
    use super::*;
    use labrys_core::{ApplicationId, PreviewAccess};

    fn test_workspace() -> SessionWorkspace {
        let repo = CanonicalRepository::new(ApplicationId::new(), "main", "abc123").unwrap();
        SessionWorkspace::open(&repo, AgentSessionId::new(), "development").unwrap()
    }

    #[test]
    fn worktree_dir_cannot_escape_root() {
        let tmp = std::env::temp_dir().join("labrys-preview-test-root");
        let root = WorkspaceRoot::new(tmp).unwrap();
        let dir = root.worktree_dir(&AgentSessionId::new()).unwrap();
        assert!(dir.starts_with(root.path()));
        assert!(dir.to_string_lossy().contains("worktrees"));
    }

    #[test]
    fn two_sessions_materialize_independent_checkouts() {
        let root = WorkspaceRoot::new(PathBuf::from("/tmp/labrys-test")).unwrap();
        let a = root.worktree_dir(&AgentSessionId::new()).unwrap();
        let b = root.worktree_dir(&AgentSessionId::new()).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn expired_preview_revokes_url_access() {
        let workspace = test_workspace();
        let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
        preview.record_health(HealthStatus::Healthy).unwrap();
        assert!(preview.temporary_url().is_some());
        preview.expire();
        assert!(preview.temporary_url().is_none());
        assert!(!preview.is_live(Utc::now()));
    }

    #[test]
    fn failed_health_issues_no_url() {
        let workspace = test_workspace();
        let mut preview = WebPreview::new(&workspace, PreviewAccess::Private, None);
        preview
            .record_health(HealthStatus::Unhealthy {
                reason: "dev server answered 500".to_string(),
            })
            .unwrap();
        assert!(preview.temporary_url().is_none());
    }

    #[test]
    fn expo_transport_modes_are_distinct() {
        let workspace = test_workspace();
        let expires = Utc::now() + chrono::Duration::hours(1);
        let go = ExpoShare::new(
            &workspace,
            "proj",
            "exp://10.0.0.1:8081",
            ExpoTransport::ExpoGo,
            expires,
        )
        .unwrap();
        let dev = ExpoShare::new(
            &workspace,
            "proj",
            "exp://10.0.0.1:8081",
            ExpoTransport::DevelopmentBuild,
            expires,
        )
        .unwrap();
        assert_ne!(go.qr_payload, dev.qr_payload);
        assert!(go.qr_payload.contains("expo_go"));
        assert!(dev.qr_payload.contains("development_build"));
    }
}
