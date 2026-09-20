use chrono::{Duration, TimeZone, Utc};

use labrys_core::inspector::ExplicitApproval;
use labrys_core::{
    AgentSessionId, ApplicationId, BuildResult, CanonicalRepository, ExpoShare, ExpoShareStatus,
    ExpoTransport, Feedback, HealthStatus, MergeRequest, PreviewAccess, PreviewStatus,
    SessionWorkspace, WebPreview, WorkspaceDiff, WorkspaceRegistry, WorktreeStatus,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn repo() -> CanonicalRepository {
    CanonicalRepository::new(ApplicationId::new(), "main", "abc123").unwrap()
}

fn diff() -> WorkspaceDiff {
    WorkspaceDiff::new(vec!["src/app.ts".to_string()], 12, 3, "add preview hook").unwrap()
}

fn workspace(session: AgentSessionId) -> SessionWorkspace {
    SessionWorkspace::open(&repo(), session, "preview").unwrap()
}

fn healthy_preview(ws: &SessionWorkspace) -> WebPreview {
    let mut preview = WebPreview::new(
        ws,
        PreviewAccess::ShareableLink,
        Some(now() + Duration::hours(1)),
    );
    preview.notify_build_started().unwrap();
    preview
        .record_build(&BuildResult::succeeded("bundle built"))
        .unwrap();
    preview.record_health(HealthStatus::Healthy).unwrap();
    preview
}

#[test]
fn two_sessions_edit_one_application_in_isolation() {
    let canonical = repo();
    let mut registry = WorkspaceRegistry::new();
    let first = AgentSessionId::new();
    let second = AgentSessionId::new();

    // WHEN two sessions run concurrently on one application ...
    let ws_a = registry.allocate(&canonical, first, "preview").unwrap();
    let ws_b = registry.allocate(&canonical, second, "preview").unwrap();

    // THEN each has an independent worktree ...
    assert_ne!(ws_a.id, ws_b.id);
    assert_ne!(ws_a.worktree_path, ws_b.worktree_path);
    assert_ne!(ws_a.branch, ws_b.branch);
    assert_eq!(ws_a.base_sha, canonical.head_sha);
    assert_eq!(ws_b.base_sha, canonical.head_sha);
    assert_eq!(
        registry
            .active_for_application(&canonical.application_id)
            .len(),
        2
    );

    // AND neither can silently overwrite the other session's changes: a second
    // allocation for the same session is rejected.
    assert!(registry.allocate(&canonical, first, "preview").is_err());
}

#[test]
fn diff_is_exposed_before_merge() {
    let mut ws = workspace(AgentSessionId::new());

    // Merge requires a reviewed diff first.
    assert!(ws.merge_ready().is_err());
    assert!(MergeRequest::propose(&ws).is_err());

    ws.record_diff(diff()).unwrap();
    let proposal = MergeRequest::propose(&ws).unwrap();
    assert_eq!(proposal.workspace_id, ws.id);
    assert!(!proposal.is_approved());

    // AND rollback metadata points at the pre-session base revision.
    let rollback = ws.rollback_info();
    assert_eq!(rollback.base_sha, ws.base_sha);
    assert_eq!(rollback.worktree_path, ws.worktree_path);
}

#[test]
fn merge_requires_named_human_approval() {
    let mut ws = workspace(AgentSessionId::new());
    ws.record_diff(diff()).unwrap();

    // Denied approval never merges.
    let denied = ExplicitApproval {
        approved: false,
        approver: "reviewer".to_string(),
        proposal_summary: "ship".to_string(),
    };
    assert!(ws.merge(&denied).is_err());
    assert_eq!(ws.status, WorktreeStatus::Active);

    // An unnamed approver is rejected.
    let unnamed = ExplicitApproval {
        approved: true,
        approver: "  ".to_string(),
        proposal_summary: "ship".to_string(),
    };
    assert!(ws.merge(&unnamed).is_err());

    let merged = ws
        .merge(&ExplicitApproval::granted(
            "reviewer",
            "ship the preview hook",
        ))
        .unwrap();
    assert_eq!(merged.approved_by.as_deref(), Some("reviewer"));
    assert_eq!(ws.status, WorktreeStatus::Merged);
    // A merged worktree is no longer mergeable and accepts no new diffs.
    assert!(ws.merge_ready().is_err());
    assert!(ws.record_diff(diff()).is_err());
}

#[test]
fn empty_diff_is_not_mergeable() {
    let mut ws = workspace(AgentSessionId::new());
    let empty = WorkspaceDiff::new(vec!["README.md".to_string()], 0, 0, "no-op touch").unwrap();
    ws.record_diff(empty).unwrap();
    assert!(ws
        .merge(&ExplicitApproval::granted("reviewer", "noop"))
        .is_err());
    assert_eq!(ws.status, WorktreeStatus::Active);
}

#[test]
fn finished_worktrees_are_cleaned_up() {
    let canonical = repo();
    let mut registry = WorkspaceRegistry::new();
    let mut ws_a = registry
        .allocate(&canonical, AgentSessionId::new(), "preview")
        .unwrap();
    let mut ws_b = registry
        .allocate(&canonical, AgentSessionId::new(), "preview")
        .unwrap();
    ws_a.record_diff(diff()).unwrap();
    ws_a.merge(&ExplicitApproval::granted("reviewer", "ship"))
        .unwrap();
    ws_b.discard().unwrap();
    registry.sync(&ws_a);
    registry.sync(&ws_b);

    assert!(ws_a.needs_cleanup());
    assert!(ws_b.needs_cleanup());
    assert_eq!(registry.cleanup_finished(), 2);
    assert!(registry.is_empty());
    // Active worktrees must not linger-claim cleanup.
    let active = registry
        .allocate(&canonical, AgentSessionId::new(), "preview")
        .unwrap();
    assert!(!active.needs_cleanup());
    assert_eq!(registry.cleanup_finished(), 0);
    assert_eq!(registry.len(), 1);
}

#[test]
fn preview_url_is_issued_only_after_health() {
    let mut preview = WebPreview::new(
        &workspace(AgentSessionId::new()),
        PreviewAccess::ShareableLink,
        Some(now() + Duration::hours(1)),
    );
    assert!(preview.temporary_url().is_none());

    preview.notify_build_started().unwrap();
    preview
        .record_build(&BuildResult::succeeded("bundle built"))
        .unwrap();
    // Dev server still starting: no URL yet.
    preview.record_health(HealthStatus::Starting).unwrap();
    assert!(preview.temporary_url().is_none());
    assert!(!preview.is_shareable());

    preview.record_health(HealthStatus::Healthy).unwrap();
    let url = preview.temporary_url().unwrap().to_string();
    assert!(url.starts_with("https://"));
    assert!(url.ends_with(".preview.labrys.local"));
    assert!(preview.is_shareable());
    assert!(preview.is_live(now()));
}

#[test]
fn preview_server_failing_health_keeps_preview_unavailable() {
    let mut preview = WebPreview::new(
        &workspace(AgentSessionId::new()),
        PreviewAccess::ShareableLink,
        Some(now() + Duration::hours(1)),
    );
    preview.notify_build_started().unwrap();
    preview
        .record_build(&BuildResult::succeeded("bundle built"))
        .unwrap();

    // WHEN the dev server starts but /health fails ...
    preview
        .record_health(HealthStatus::Unhealthy {
            reason: "/health returned 503".to_string(),
        })
        .unwrap();

    // THEN the preview remains unavailable ...
    assert!(matches!(preview.status, PreviewStatus::Unavailable { .. }));
    assert!(preview.temporary_url().is_none());
    assert!(!preview.is_shareable());
    assert!(!preview.is_live(now()));

    // AND the failure is visible in preview and agent events.
    let detail = preview.to_event_detail();
    assert!(detail.contains("unavailable"));
    assert!(detail.contains("/health returned 503"));
}

#[test]
fn failed_preview_build_is_visible_with_limit_and_recovery() {
    let mut preview = WebPreview::new(
        &workspace(AgentSessionId::new()),
        PreviewAccess::Private,
        None,
    );
    preview.notify_build_started().unwrap();
    preview
        .record_build(&BuildResult::timed_out(600, 601))
        .unwrap();

    assert!(matches!(preview.status, PreviewStatus::Unavailable { .. }));
    let error = preview.build_error.clone().unwrap();
    assert!(error.contains("cancelled"));
    assert!(error.contains("timeout_secs=600"));
    assert!(error.contains("recovery:"));
    assert!(preview.to_event_detail().contains(&error));

    // An unavailable preview cannot start another build.
    assert!(preview.notify_build_started().is_err());
}

#[test]
fn private_previews_are_never_shareable() {
    let preview = {
        let mut p = WebPreview::new(
            &workspace(AgentSessionId::new()),
            PreviewAccess::Private,
            None,
        );
        p.record_health(HealthStatus::Healthy).unwrap();
        p
    };
    assert_eq!(preview.status, PreviewStatus::Available);
    assert!(preview.temporary_url().is_some());
    assert!(!preview.is_shareable());
}

#[test]
fn expiry_revokes_preview_url() {
    let mut preview = healthy_preview(&workspace(AgentSessionId::new()));
    assert!(preview.is_live(now()));
    assert!(!preview.is_live(now() + Duration::hours(2)));

    preview.expire();
    assert_eq!(preview.status, PreviewStatus::Expired);
    assert!(preview.temporary_url().is_none());
    assert!(!preview.is_live(now()));
    // Expired previews reject further health records.
    assert!(preview.record_health(HealthStatus::Healthy).is_err());
}

#[test]
fn expo_qr_resolves_to_session_dev_server() {
    let session = AgentSessionId::new();
    let ws = workspace(session);
    let server = ExpoShare::discover_server_url("10.0.0.5", 8081).unwrap();
    assert_eq!(server, "exp://10.0.0.5:8081");
    assert!(ExpoShare::discover_server_url("  ", 8081).is_err());
    assert!(ExpoShare::discover_server_url("10.0.0.5", 0).is_err());

    // WHEN an active Expo Go preview is shared ...
    let share = ExpoShare::new(
        &ws,
        "labrys-demo",
        &server,
        ExpoTransport::ExpoGo,
        now() + Duration::hours(4),
    )
    .unwrap();

    // THEN the QR resolves to the session's development server ...
    assert!(share.qr_payload.starts_with(&format!("{server}/--/expo?")));
    assert!(share.qr_payload.contains("project=labrys-demo"));
    assert!(share.qr_payload.contains(&format!("session={session}")));
    assert!(share.qr_payload.contains("transport=expo_go"));
    assert!(share.transport.is_expo_go());

    // AND the share record indicates its expiry and transport mode.
    assert!(share.is_active(now()));
    assert!(!share.is_active(now() + Duration::hours(5)));
    let detail = share.to_event_detail();
    assert!(detail.contains(&share.expires_at.to_rfc3339()));
    assert!(detail.contains("transport expo_go"));
}

#[test]
fn expo_development_build_is_a_distinct_transport() {
    let ws = workspace(AgentSessionId::new());
    let mut share = ExpoShare::new(
        &ws,
        "labrys-demo",
        "exp://10.0.0.5:8081",
        ExpoTransport::DevelopmentBuild,
        now() + Duration::hours(4),
    )
    .unwrap();
    assert!(!share.transport.is_expo_go());
    assert_eq!(share.transport.canonical_name(), "development_build");
    assert!(share.qr_payload.contains("transport=development_build"));

    share.expire();
    // Revoked/expired shares stop resolving to the dev server.
    assert!(!share.is_active(now()));
}

#[test]
fn expo_share_revocation_stops_access() {
    let mut share = ExpoShare::new(
        &workspace(AgentSessionId::new()),
        "labrys-demo",
        "exp://10.0.0.5:8081",
        ExpoTransport::ExpoGo,
        now() + Duration::hours(4),
    )
    .unwrap();
    assert_eq!(share.status, ExpoShareStatus::Active);
    share.revoke();
    assert_eq!(share.status, ExpoShareStatus::Revoked);
    assert!(!share.is_active(now()));
}

#[test]
fn feedback_is_associated_with_its_workspace_and_preview() {
    let ws = workspace(AgentSessionId::new());
    let preview = healthy_preview(&ws);

    // Workspace-level feedback.
    let general = Feedback::new(&ws, "reviewer", "spacing looks off", now()).unwrap();
    assert_eq!(general.workspace_id, ws.id);
    assert_eq!(general.session_id, ws.session_id);
    assert!(general.preview_id.is_none());

    // Feedback attached to a preview in the same workspace.
    let attached =
        Feedback::for_web_preview(&ws, &preview, "reviewer", "header overlaps", now()).unwrap();
    assert_eq!(attached.preview_id, Some(preview.id));

    // Cross-workspace association is rejected.
    let other_ws = workspace(AgentSessionId::new());
    assert!(
        Feedback::for_web_preview(&other_ws, &preview, "reviewer", "wrong workspace", now())
            .is_err()
    );

    let share = ExpoShare::new(
        &other_ws,
        "labrys-demo",
        "exp://10.0.0.5:8081",
        ExpoTransport::ExpoGo,
        now() + Duration::hours(4),
    )
    .unwrap();
    let mobile =
        Feedback::for_expo_share(&other_ws, &share, "reviewer", "button too small", now()).unwrap();
    assert_eq!(mobile.preview_id, Some(share.id));
    assert!(Feedback::for_expo_share(&ws, &share, "reviewer", "wrong workspace", now()).is_err());

    // Author and body are required.
    assert!(Feedback::new(&ws, "  ", "body", now()).is_err());
    assert!(Feedback::new(&ws, "reviewer", "  ", now()).is_err());
}

#[test]
fn workspace_contracts_round_trip_as_strict_json() {
    let ws = workspace(AgentSessionId::new());
    let restored = SessionWorkspace::from_json(&ws.to_json().unwrap()).unwrap();
    assert_eq!(restored, ws);

    let preview = healthy_preview(&ws);
    let restored = WebPreview::from_json(&preview.to_json().unwrap()).unwrap();
    assert_eq!(restored, preview);

    let share = ExpoShare::new(
        &ws,
        "labrys-demo",
        "exp://10.0.0.5:8081",
        ExpoTransport::ExpoGo,
        now() + Duration::hours(4),
    )
    .unwrap();
    let restored = ExpoShare::from_json(&share.to_json().unwrap()).unwrap();
    assert_eq!(restored, share);

    let feedback = Feedback::new(&ws, "reviewer", "lgtm", now()).unwrap();
    let restored = Feedback::from_json(&feedback.to_json().unwrap()).unwrap();
    assert_eq!(restored, feedback);

    let mut merge = MergeRequest::propose(&{
        let mut w = ws.clone();
        w.record_diff(diff()).unwrap();
        w
    })
    .unwrap();
    merge
        .approve(&ExplicitApproval::granted("reviewer", "ship"))
        .unwrap();
    assert!(merge.is_approved());
    let restored = MergeRequest::from_json(&merge.to_json().unwrap()).unwrap();
    assert_eq!(restored, merge);

    // Unknown fields are rejected.
    let mut value: serde_json::Value = serde_json::from_str(&ws.to_json().unwrap()).unwrap();
    value["unknown_future_field"] = serde_json::json!("nope");
    assert!(SessionWorkspace::from_json(&value.to_string()).is_err());
}
