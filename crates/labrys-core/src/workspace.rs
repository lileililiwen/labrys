//! Deterministic workspace and preview model.
//!
//! Covers requirement `preview-platform`: every agent session modifies an
//! isolated Git worktree and exposes its diff before merge; web previews issue
//! a temporary URL only after the development runtime is healthy; Expo mobile
//! previews expose project, server, expiry, and QR transport metadata with an
//! explicit Expo Go versus development-build distinction.
//!
//! This module is pure and deterministic: it plans worktree paths, tracks
//! diff/rollback metadata, gates preview URLs on health, and renders event
//! detail for preview and agent streams. It never touches the filesystem,
//! spawns a proxy, or contacts an Expo server; that I/O belongs to
//! infrastructure built on these plans.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::ids::{AgentSessionId, ApplicationId, FeedbackId, PreviewId, WorkspaceId};
use crate::inspector::ExplicitApproval;
use crate::runtime::{BuildResult, HealthStatus};

/// Canonical repository a workspace is derived from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRepository {
    pub application_id: ApplicationId,
    pub default_branch: String,
    pub head_sha: String,
}

impl CanonicalRepository {
    pub fn new(
        application_id: ApplicationId,
        default_branch: impl Into<String>,
        head_sha: impl Into<String>,
    ) -> Result<Self> {
        let default_branch = default_branch.into();
        let head_sha = head_sha.into();
        if default_branch.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "canonical repository requires a default branch".to_string(),
            ));
        }
        if head_sha.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "canonical repository requires a head sha".to_string(),
            ));
        }
        Ok(Self {
            application_id,
            default_branch,
            head_sha,
        })
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Lifecycle of one isolated worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WorktreeStatus {
    Active,
    Merged,
    Discarded,
}

/// File diff exposed before merge, plus rollback metadata linkage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDiff {
    #[serde(default)]
    pub files_changed: Vec<String>,
    pub additions: u32,
    pub deletions: u32,
    pub summary: String,
}

impl WorkspaceDiff {
    pub fn new(
        files_changed: Vec<String>,
        additions: u32,
        deletions: u32,
        summary: impl Into<String>,
    ) -> Result<Self> {
        let summary = summary.into();
        if files_changed.is_empty() {
            return Err(CoreError::WorkspaceState(
                "workspace diff must name at least one changed file".to_string(),
            ));
        }
        if summary.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "workspace diff requires a summary".to_string(),
            ));
        }
        Ok(Self {
            files_changed,
            additions,
            deletions,
            summary,
        })
    }

    pub fn is_empty_change(&self) -> bool {
        self.additions == 0 && self.deletions == 0
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Rollback metadata: everything needed to restore the base revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackInfo {
    pub base_sha: String,
    pub base_branch: String,
    pub worktree_path: String,
}

/// One agent session's isolated worktree.
///
/// Isolation is structural: the worktree path and branch embed the session id,
/// so two sessions on one application can never share a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionWorkspace {
    pub id: WorkspaceId,
    pub application_id: ApplicationId,
    pub environment_name: String,
    pub session_id: AgentSessionId,
    pub worktree_path: String,
    pub branch: String,
    pub base_sha: String,
    pub status: WorktreeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<WorkspaceDiff>,
}

impl SessionWorkspace {
    /// Derives an isolated worktree from the canonical repository.
    pub fn open(
        repo: &CanonicalRepository,
        session_id: AgentSessionId,
        environment_name: impl Into<String>,
    ) -> Result<Self> {
        let environment_name = environment_name.into();
        if environment_name.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "workspace requires an environment name".to_string(),
            ));
        }
        Ok(Self {
            id: WorkspaceId::new(),
            application_id: repo.application_id,
            environment_name,
            session_id,
            worktree_path: format!("worktrees/{session_id}"),
            branch: format!("session/{session_id}"),
            base_sha: repo.head_sha.clone(),
            status: WorktreeStatus::Active,
            diff: None,
        })
    }

    /// Records the session's file changes. Only active worktrees accept diffs.
    pub fn record_diff(&mut self, diff: WorkspaceDiff) -> Result<()> {
        if self.status != WorktreeStatus::Active {
            return Err(CoreError::WorkspaceState(format!(
                "cannot record diff while workspace {} is {:?}",
                self.id, self.status
            )));
        }
        self.diff = Some(diff);
        Ok(())
    }

    /// Rollback metadata pointing at the pre-session base revision.
    pub fn rollback_info(&self) -> RollbackInfo {
        RollbackInfo {
            base_sha: self.base_sha.clone(),
            base_branch: self.branch.clone(),
            worktree_path: self.worktree_path.clone(),
        }
    }

    /// Returns the diff that must be reviewed before merge.
    pub fn merge_ready(&self) -> Result<&WorkspaceDiff> {
        if self.status != WorktreeStatus::Active {
            return Err(CoreError::WorkspaceState(format!(
                "workspace {} is {:?}, not mergeable",
                self.id, self.status
            )));
        }
        self.diff.as_ref().ok_or_else(|| {
            CoreError::WorkspaceState(format!(
                "workspace {} has no exposed diff; merge requires review",
                self.id
            ))
        })
    }

    /// Merges the reviewed diff into the canonical repository record.
    ///
    /// Requires an exposed non-empty diff and a granted, named human approval.
    pub fn merge(&mut self, approval: &ExplicitApproval) -> Result<MergeRequest> {
        let diff = self.merge_ready()?.clone();
        if diff.is_empty_change() {
            return Err(CoreError::WorkspaceState(format!(
                "workspace {} diff is empty; nothing to merge",
                self.id
            )));
        }
        if !approval.approved {
            return Err(CoreError::ApprovalRequired(format!(
                "workspace {} merge requires explicit approval",
                self.id
            )));
        }
        if approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(
                "workspace merge approval must name an approver".to_string(),
            ));
        }
        self.status = WorktreeStatus::Merged;
        Ok(MergeRequest {
            workspace_id: self.id,
            diff,
            approved_by: Some(approval.approver.clone()),
        })
    }

    /// Discards the worktree without merging (cleanup path).
    pub fn discard(&mut self) -> Result<()> {
        if self.status != WorktreeStatus::Active {
            return Err(CoreError::WorkspaceState(format!(
                "cannot discard workspace {} while {:?}",
                self.id, self.status
            )));
        }
        self.status = WorktreeStatus::Discarded;
        self.diff = None;
        Ok(())
    }

    /// True once the worktree is merged or discarded and its checkout can be
    /// removed. Preview environments are disposable and environment scoped, so
    /// finished worktrees must not linger.
    pub fn needs_cleanup(&self) -> bool {
        self.status != WorktreeStatus::Active
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Merge contract: a reviewed diff plus the approver who accepted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeRequest {
    pub workspace_id: WorkspaceId,
    pub diff: WorkspaceDiff,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

impl MergeRequest {
    /// Proposes a merge from a workspace that already exposes its diff.
    pub fn propose(workspace: &SessionWorkspace) -> Result<Self> {
        let diff = workspace.merge_ready()?.clone();
        Ok(Self {
            workspace_id: workspace.id,
            diff,
            approved_by: None,
        })
    }

    /// Records the human approval. Rejects denials and unnamed approvers.
    pub fn approve(&mut self, approval: &ExplicitApproval) -> Result<()> {
        if !approval.approved {
            return Err(CoreError::ApprovalRequired(
                "merge proposal was denied".to_string(),
            ));
        }
        if approval.approver.trim().is_empty() {
            return Err(CoreError::ApprovalRequired(
                "merge approval must name an approver".to_string(),
            ));
        }
        self.approved_by = Some(approval.approver.clone());
        Ok(())
    }

    pub fn is_approved(&self) -> bool {
        self.approved_by.is_some()
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Tracks allocated worktrees so concurrent sessions stay isolated and
/// finished checkouts are cleaned up.
#[derive(Debug, Default)]
pub struct WorkspaceRegistry {
    workspaces: Vec<SessionWorkspace>,
}

impl WorkspaceRegistry {
    pub fn new() -> Self {
        Self {
            workspaces: Vec::new(),
        }
    }

    /// Allocates one worktree per session. A second allocation for the same
    /// session is rejected so a session can never silently overwrite itself.
    pub fn allocate(
        &mut self,
        repo: &CanonicalRepository,
        session_id: AgentSessionId,
        environment_name: impl Into<String>,
    ) -> Result<SessionWorkspace> {
        if self
            .workspaces
            .iter()
            .any(|w| w.session_id == session_id && w.application_id == repo.application_id)
        {
            return Err(CoreError::WorkspaceState(format!(
                "session {session_id} already owns a worktree"
            )));
        }
        let workspace = SessionWorkspace::open(repo, session_id, environment_name)?;
        self.workspaces.push(workspace.clone());
        Ok(workspace)
    }

    pub fn get(&self, id: &WorkspaceId) -> Option<&SessionWorkspace> {
        self.workspaces.iter().find(|w| &w.id == id)
    }

    pub fn active_for_application(&self, application_id: &ApplicationId) -> Vec<&SessionWorkspace> {
        self.workspaces
            .iter()
            .filter(|w| &w.application_id == application_id && w.status == WorktreeStatus::Active)
            .collect()
    }

    /// Syncs a mutated workspace back into the registry.
    pub fn sync(&mut self, workspace: &SessionWorkspace) {
        if let Some(slot) = self.workspaces.iter_mut().find(|w| w.id == workspace.id) {
            *slot = workspace.clone();
        }
    }

    /// Removes merged/discarded worktrees. Returns the removed count.
    pub fn cleanup_finished(&mut self) -> usize {
        let before = self.workspaces.len();
        self.workspaces.retain(|w| !w.needs_cleanup());
        before - self.workspaces.len()
    }

    pub fn len(&self) -> usize {
        self.workspaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }
}

/// Who may open a web preview URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PreviewAccess {
    #[default]
    Private,
    ShareableLink,
}

/// Lifecycle of a web preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PreviewStatus {
    Pending,
    Building,
    Available,
    Unavailable { reason: String },
    Expired,
}

/// Shareable web preview for one isolated worktree.
///
/// The temporary URL exists only while the preview is `Available`; health or
/// build failures move it to `Unavailable` and revoke URL access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebPreview {
    pub id: PreviewId,
    pub workspace_id: WorkspaceId,
    pub session_id: AgentSessionId,
    pub access: PreviewAccess,
    pub status: PreviewStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl WebPreview {
    pub fn new(
        workspace: &SessionWorkspace,
        access: PreviewAccess,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: PreviewId::new(),
            workspace_id: workspace.id,
            session_id: workspace.session_id,
            access,
            status: PreviewStatus::Pending,
            health: None,
            url: None,
            build_error: None,
            expires_at,
        }
    }

    /// Temporary reverse-proxy URL. `Some` only while `Available`.
    pub fn temporary_url(&self) -> Option<&str> {
        if self.status == PreviewStatus::Available {
            self.url.as_deref()
        } else {
            None
        }
    }

    /// True when the preview is available and its access policy allows sharing.
    pub fn is_shareable(&self) -> bool {
        self.status == PreviewStatus::Available && self.access == PreviewAccess::ShareableLink
    }

    /// Marks the start of a preview build. Only pending previews can start.
    pub fn notify_build_started(&mut self) -> Result<()> {
        if self.status != PreviewStatus::Pending {
            return Err(CoreError::PreviewUnavailable(format!(
                "preview {} cannot start a build while {:?}",
                self.id, self.status
            )));
        }
        self.status = PreviewStatus::Building;
        Ok(())
    }

    /// Records the preview build outcome. Failures and cancellations make the
    /// preview unavailable with the enforced limit and recovery detail, so the
    /// failure is visible in both preview and agent events.
    pub fn record_build(&mut self, result: &BuildResult) -> Result<()> {
        if matches!(
            self.status,
            PreviewStatus::Available | PreviewStatus::Expired
        ) {
            return Err(CoreError::PreviewUnavailable(format!(
                "preview {} cannot record a build while {:?}",
                self.id, self.status
            )));
        }
        if result.is_success() {
            if self.status == PreviewStatus::Pending {
                self.status = PreviewStatus::Building;
            }
            self.build_error = None;
            return Ok(());
        }
        let mut reason = format!(
            "preview build {}: {}",
            self.status_label(result),
            result.detail
        );
        if let Some(limit) = &result.limit_enforced {
            reason.push_str(&format!(" (limit {limit})"));
        }
        if let Some(recovery) = &result.recovery {
            reason.push_str(&format!("; recovery: {recovery}"));
        }
        self.build_error = Some(reason.clone());
        self.status = PreviewStatus::Unavailable { reason };
        Ok(())
    }

    /// Records the dev-runtime health outcome. A healthy runtime issues the
    /// temporary URL; any other health keeps the preview unavailable.
    pub fn record_health(&mut self, health: HealthStatus) -> Result<()> {
        if self.status == PreviewStatus::Expired {
            return Err(CoreError::PreviewUnavailable(format!(
                "preview {} is expired",
                self.id
            )));
        }
        self.health = Some(health.clone());
        match health {
            HealthStatus::Healthy => {
                if self.url.is_none() {
                    self.url = Some(format!("https://{}.preview.labrys.local", self.id));
                }
                self.status = PreviewStatus::Available;
            }
            HealthStatus::Starting => {
                if self.status == PreviewStatus::Pending {
                    self.status = PreviewStatus::Building;
                }
            }
            HealthStatus::Unhealthy { reason } | HealthStatus::Unknown { reason } => {
                self.status = PreviewStatus::Unavailable {
                    reason: format!("preview health failed: {reason}"),
                };
            }
        }
        Ok(())
    }

    /// Renders the preview detail shared by preview and agent event streams.
    pub fn to_event_detail(&self) -> String {
        match &self.status {
            PreviewStatus::Available => format!(
                "preview {} for workspace {} available at {}",
                self.id,
                self.workspace_id,
                self.url.as_deref().unwrap_or("<pending url>")
            ),
            PreviewStatus::Unavailable { reason } => format!(
                "preview {} for workspace {} unavailable: {reason}",
                self.id, self.workspace_id
            ),
            other => format!(
                "preview {} for workspace {} {other:?}",
                self.id, self.workspace_id
            ),
        }
    }

    /// Expires the preview and revokes URL access.
    pub fn expire(&mut self) {
        self.status = PreviewStatus::Expired;
    }

    /// True while the preview may serve traffic at `now`.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        if self.status != PreviewStatus::Available {
            return false;
        }
        match self.expires_at {
            Some(expiry) => now < expiry,
            None => true,
        }
    }

    fn status_label(&self, result: &BuildResult) -> &'static str {
        if result.cancelled {
            "cancelled"
        } else {
            "failed"
        }
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Mobile preview transport. Expo Go and development builds are distinct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpoTransport {
    #[default]
    ExpoGo,
    DevelopmentBuild,
}

impl ExpoTransport {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::ExpoGo => "expo_go",
            Self::DevelopmentBuild => "development_build",
        }
    }

    pub fn is_expo_go(self) -> bool {
        matches!(self, Self::ExpoGo)
    }
}

/// Lifecycle of a mobile share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpoShareStatus {
    Active,
    Expired,
    Revoked,
}

/// Expo mobile preview share: project, dev server, expiry, and QR transport
/// metadata. The QR payload always resolves to the session's development
/// server and names its transport mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpoShare {
    pub id: PreviewId,
    pub workspace_id: WorkspaceId,
    pub session_id: AgentSessionId,
    pub project_slug: String,
    pub dev_server_url: String,
    pub transport: ExpoTransport,
    pub status: ExpoShareStatus,
    pub expires_at: DateTime<Utc>,
    pub qr_payload: String,
}

impl ExpoShare {
    /// Discovers the Expo dev-server URL for a host and port.
    pub fn discover_server_url(host: impl Into<String>, port: u16) -> Result<String> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(CoreError::PreviewUnavailable(
                "expo server discovery requires a host".to_string(),
            ));
        }
        if port == 0 {
            return Err(CoreError::PreviewUnavailable(
                "expo server discovery requires a port in 1..=65535".to_string(),
            ));
        }
        Ok(format!("exp://{host}:{port}"))
    }

    /// Creates a share with a QR payload bound to the session's dev server.
    pub fn new(
        workspace: &SessionWorkspace,
        project_slug: impl Into<String>,
        dev_server_url: impl Into<String>,
        transport: ExpoTransport,
        expires_at: DateTime<Utc>,
    ) -> Result<Self> {
        let project_slug = project_slug.into();
        let dev_server_url = dev_server_url.into();
        if project_slug.trim().is_empty() {
            return Err(CoreError::PreviewUnavailable(
                "expo share requires a project slug".to_string(),
            ));
        }
        if dev_server_url.trim().is_empty() {
            return Err(CoreError::PreviewUnavailable(
                "expo share requires a dev server url".to_string(),
            ));
        }
        let id = PreviewId::new();
        let qr_payload = Self::qr_payload_for(
            &dev_server_url,
            &project_slug,
            &workspace.session_id,
            transport,
        );
        Ok(Self {
            id,
            workspace_id: workspace.id,
            session_id: workspace.session_id,
            project_slug,
            dev_server_url,
            transport,
            status: ExpoShareStatus::Active,
            expires_at,
            qr_payload,
        })
    }

    fn qr_payload_for(
        dev_server_url: &str,
        project_slug: &str,
        session_id: &AgentSessionId,
        transport: ExpoTransport,
    ) -> String {
        format!(
            "{dev_server_url}/--/expo?project={project_slug}&session={session_id}&transport={}",
            transport.canonical_name()
        )
    }

    /// True while the share is active and unexpired at `now`.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.status == ExpoShareStatus::Active && now < self.expires_at
    }

    pub fn expire(&mut self) {
        self.status = ExpoShareStatus::Expired;
    }

    pub fn revoke(&mut self) {
        self.status = ExpoShareStatus::Revoked;
    }

    /// Renders the share detail with expiry and transport mode.
    pub fn to_event_detail(&self) -> String {
        format!(
            "expo share {} project '{}' server {} expires {} transport {} status {:?}",
            self.id,
            self.project_slug,
            self.dev_server_url,
            self.expires_at.to_rfc3339(),
            self.transport.canonical_name(),
            self.status
        )
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}

/// Human feedback attached to a workspace and optionally one preview.
///
/// The preview, when present, must belong to the same workspace; orphan or
/// cross-workspace feedback is rejected so review comments always land on the
/// change they describe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feedback {
    pub id: FeedbackId,
    pub workspace_id: WorkspaceId,
    pub session_id: AgentSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<PreviewId>,
    pub author: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

impl Feedback {
    pub fn new(
        workspace: &SessionWorkspace,
        author: impl Into<String>,
        body: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        let author = author.into();
        let body = body.into();
        if author.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "feedback requires an author".to_string(),
            ));
        }
        if body.trim().is_empty() {
            return Err(CoreError::WorkspaceState(
                "feedback requires a body".to_string(),
            ));
        }
        Ok(Self {
            id: FeedbackId::new(),
            workspace_id: workspace.id,
            session_id: workspace.session_id,
            preview_id: None,
            author,
            body,
            created_at,
        })
    }

    /// Attaches feedback to a web preview in the same workspace.
    pub fn for_web_preview(
        workspace: &SessionWorkspace,
        preview: &WebPreview,
        author: impl Into<String>,
        body: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        if preview.workspace_id != workspace.id {
            return Err(CoreError::WorkspaceState(format!(
                "feedback preview {} does not belong to workspace {}",
                preview.id, workspace.id
            )));
        }
        let mut feedback = Self::new(workspace, author, body, created_at)?;
        feedback.preview_id = Some(preview.id);
        Ok(feedback)
    }

    /// Attaches feedback to an Expo share in the same workspace.
    pub fn for_expo_share(
        workspace: &SessionWorkspace,
        share: &ExpoShare,
        author: impl Into<String>,
        body: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Result<Self> {
        if share.workspace_id != workspace.id {
            return Err(CoreError::WorkspaceState(format!(
                "feedback expo share {} does not belong to workspace {}",
                share.id, workspace.id
            )));
        }
        let mut feedback = Self::new(workspace, author, body, created_at)?;
        feedback.preview_id = Some(share.id);
        Ok(feedback)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(CoreError::from)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(CoreError::from)
    }
}
