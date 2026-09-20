use chrono::{TimeZone, Utc};
use labrys_core::{
    ensure_protocol_version, select_preferred_tool, ActionKind, AgentBackend, AgentContext,
    AgentEvent, AgentEventKind, AgentInput, AgentSession, ApplicationId, BackendIdentity,
    CapabilityRequest, ExplicitApproval, ExternalProcessAgent, NativeAgent, NativePhase, Policy,
    ResourceOperation, ResourceRequest, SecretReference, SessionStatus, ToolTier,
    AGENT_PROTOCOL_VERSION, NORMALIZED_EVENT_NAMES,
};

fn context(production: bool) -> AgentContext {
    AgentContext::new(
        ApplicationId::new(),
        if production {
            "production"
        } else {
            "development"
        },
        production,
        "human:operator",
    )
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

#[test]
fn versioned_contract_rejects_mismatched_adapters() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 1);
    assert!(ensure_protocol_version(1).is_ok());
    assert!(ensure_protocol_version(999).is_err());

    // An adapter speaking another protocol version cannot open a session.
    let stale = ExternalProcessAgent::new("stale-driver", 999, vec!["agent".to_string()]);
    let err = AgentSession::open(&stale, context(false), Vec::new()).unwrap_err();
    assert!(
        matches!(err, labrys_core::CoreError::ProtocolVersion { .. }),
        "expected protocol version error, got {err:?}"
    );
}

#[test]
fn native_and_external_sessions_emit_one_event_vocabulary() {
    // WHEN equivalent actions occur through two backends ...
    let mut native_backend = NativeAgent::new();
    let mut native_session =
        AgentSession::open(&native_backend, context(false), Vec::new()).unwrap();
    let native_events = native_session
        .prompt(&mut native_backend, &AgentInput::new("ship it"), now())
        .unwrap();

    let mut external_backend = ExternalProcessAgent::new(
        "opencode",
        AGENT_PROTOCOL_VERSION,
        vec!["opencode".to_string()],
    )
    .with_scripted(vec![
        (
            AgentEventKind::Planning,
            "external planned the prompt".to_string(),
        ),
        (AgentEventKind::CommandRun, "external ran tests".to_string()),
        (
            AgentEventKind::Verification,
            "external verified output".to_string(),
        ),
    ]);
    let mut external_session =
        AgentSession::open(&external_backend, context(false), Vec::new()).unwrap();
    let external_events = external_session
        .prompt(&mut external_backend, &AgentInput::new("ship it"), now())
        .unwrap();

    // THEN the platform records normalized events with backend identity ...
    for event in native_events.iter().chain(external_events.iter()) {
        assert!(
            NORMALIZED_EVENT_NAMES.contains(&event.kind.canonical_name()),
            "event kind must be in the normalized vocabulary"
        );
    }
    assert!(
        native_events
            .iter()
            .all(|e| e.backend == BackendIdentity::native()),
        "native events carry native identity"
    );
    assert!(
        external_events
            .iter()
            .all(|e| e.backend == BackendIdentity::external("opencode")),
        "external events carry their backend identity"
    );
    // AND consumers do not depend on backend-specific event names.
    assert_eq!(
        native_events.first().map(|e| e.kind),
        Some(AgentEventKind::Planning)
    );
    assert_eq!(
        external_events.first().map(|e| e.kind),
        Some(AgentEventKind::Planning)
    );
    // Sequences are contiguous from zero within each session.
    for (i, event) in native_events.iter().enumerate() {
        assert_eq!(event.sequence, i as u64);
    }
}

#[test]
fn event_ordering_is_enforced_and_replayable() {
    let mut backend = NativeAgent::new();
    let mut session = AgentSession::open(&backend, context(false), Vec::new()).unwrap();
    let first = session
        .prompt(&mut backend, &AgentInput::new("first"), now())
        .unwrap();
    let second = session
        .prompt(&mut backend, &AgentInput::new("second"), now())
        .unwrap();
    let expected: Vec<u64> = (0..(first.len() + second.len()) as u64).collect();
    let actual: Vec<u64> = session.events().iter().map(|e| e.sequence).collect();
    assert_eq!(actual, expected, "sequences must be contiguous");

    // Replaying an out-of-order event is rejected.
    let mut tampered = session.events()[0].clone();
    tampered.sequence = 999;
    assert!(session.replay_event(tampered).is_err());

    // Replaying a foreign session's event is rejected.
    let mut other_backend = NativeAgent::new();
    let mut other = AgentSession::open(&other_backend, context(false), Vec::new()).unwrap();
    other
        .prompt(&mut other_backend, &AgentInput::new("other"), now())
        .unwrap();
    let foreign = other.events()[0].clone();
    assert!(session.replay_event(foreign).is_err());

    // Strict JSON roundtrip; unknown fields are rejected.
    let event = &session.events()[0];
    let restored = AgentEvent::from_json(&event.to_json().unwrap()).unwrap();
    assert_eq!(&restored, event);
    let mut value: serde_json::Value = serde_json::from_str(&event.to_json().unwrap()).unwrap();
    value["unknown_future_field"] = serde_json::json!("nope");
    assert!(AgentEvent::from_json(&serde_json::to_string(&value).unwrap()).is_err());
}

#[test]
fn denied_actions_record_denial_without_executing() {
    let mut backend = NativeAgent::new();
    let mut session = AgentSession::open(&backend, context(true), Vec::new()).unwrap();

    // Raw shell is denied in production at the platform boundary.
    let denied = session
        .prompt(
            &mut backend,
            &AgentInput::new("run it live")
                .with_action(ActionKind::ReadOnly)
                .with_tool(ToolTier::RawShell),
            now(),
        )
        .unwrap_err();
    assert!(
        matches!(denied, labrys_core::CoreError::PolicyDenied(_)),
        "expected policy denial, got {denied:?}"
    );
    assert_eq!(
        session.events().last().map(|e| e.kind),
        Some(AgentEventKind::Denied),
        "denial must be observable in the stream"
    );
    // Policy denial leaves the session usable for allowed tools.
    let ok = session
        .prompt(
            &mut backend,
            &AgentInput::new("inspect")
                .with_action(ActionKind::ReadOnly)
                .with_tool(ToolTier::PlatformApi),
            now(),
        )
        .unwrap();
    assert!(!ok.is_empty());
}

#[test]
fn production_deploy_waits_for_human_approval() {
    let mut backend = NativeAgent::new();
    let mut session = AgentSession::open(&backend, context(true), Vec::new()).unwrap();

    // WHEN an agent requests production deployment ...
    let blocked = session
        .prompt(
            &mut backend,
            &AgentInput::new("deploy to prod").with_action(ActionKind::ProductionDeploy),
            now(),
        )
        .unwrap_err();
    assert!(
        matches!(blocked, labrys_core::CoreError::ApprovalRequired(_)),
        "expected approval gate, got {blocked:?}"
    );
    // THEN validation creates an approval request ...
    assert_eq!(session.pending_approvals().len(), 1);
    let request_id = session.pending_approvals()[0].id.clone();
    assert_eq!(
        session.events().last().map(|e| e.kind),
        Some(AgentEventKind::ApprovalRequested)
    );
    // AND execution cannot start before approval: denied stays denied.
    let denied_event = session
        .resolve_approval(
            &request_id,
            &ExplicitApproval::denied("human:reviewer", "too risky"),
        )
        .unwrap();
    assert_eq!(denied_event.kind, AgentEventKind::Denied);
    assert!(session.pending_approvals().is_empty());

    // A granted approval resolves the request; unnamed approvers are rejected.
    let _ = session
        .prompt(
            &mut backend,
            &AgentInput::new("deploy to prod").with_action(ActionKind::ProductionDeploy),
            now(),
        )
        .unwrap_err();
    let request_id = session.pending_approvals()[0].id.clone();
    assert!(session
        .resolve_approval(&request_id, &ExplicitApproval::granted("", "prod deploy"))
        .is_err());
    let granted_event = session
        .resolve_approval(
            &request_id,
            &ExplicitApproval::granted("human:reviewer", "prod deploy"),
        )
        .unwrap();
    assert_eq!(granted_event.kind, AgentEventKind::Verification);

    // Unknown approval requests cannot be resolved.
    assert!(session
        .resolve_approval("apr_missing", &ExplicitApproval::granted("human:x", "y"))
        .is_err());

    // Every high-risk action variant requires approval, even outside production.
    for action in [
        ActionKind::DestructiveResource,
        ActionKind::DestructiveMigration,
        ActionKind::SecretReplacement,
        ActionKind::DomainChange,
        ActionKind::BillingChange,
        ActionKind::ExternalWrite,
    ] {
        let mut backend = NativeAgent::new();
        let mut session = AgentSession::open(&backend, context(false), Vec::new()).unwrap();
        let err = session
            .prompt(
                &mut backend,
                &AgentInput::new("risky").with_action(action),
                now(),
            )
            .unwrap_err();
        assert!(matches!(err, labrys_core::CoreError::ApprovalRequired(_)));
    }
}

#[test]
fn secrets_stay_out_of_agent_context() {
    const SECRET: &str = "super-secret-value-123";
    let mut backend = NativeAgent::new();
    let mut session =
        AgentSession::open(&backend, context(false), vec![SECRET.to_string()]).unwrap();

    // WHEN a prompt carries a secret value it is rejected ...
    let leak = session
        .prompt(
            &mut backend,
            &AgentInput::new(format!("token is {SECRET}")),
            now(),
        )
        .unwrap_err();
    assert!(matches!(leak, labrys_core::CoreError::SecretLeak(_)));

    // WHEN a binding requests a secret, the runtime gets a reference and the
    // agent stream contains only the reference identifier.
    let request = CapabilityRequest {
        capability: "postgres".to_string(),
        action: "connect".to_string(),
        secret_refs: vec![SecretReference::new("secret://prod/db-password")],
        reason: "runtime needs a client secret".to_string(),
    };
    let detail = request.to_event_detail();
    assert!(!detail.contains(SECRET));
    assert!(detail.contains("secret://prod/db-password"));

    let resource = ResourceRequest {
        resource: "db".to_string(),
        operation: ResourceOperation::Read,
        secret_refs: vec![SecretReference::new("secret://prod/db-password")],
        reason: "migration check".to_string(),
    };
    assert!(resource
        .to_event_detail()
        .contains("secret://prod/db-password"));

    // Backend emissions carrying secret values are rejected too.
    let mut leaky = ExternalProcessAgent::new("leaky", AGENT_PROTOCOL_VERSION, Vec::new())
        .with_scripted(vec![(AgentEventKind::Planning, format!("oops {SECRET}"))]);
    let mut leaky_session =
        AgentSession::open(&leaky, context(false), vec![SECRET.to_string()]).unwrap();
    let err = leaky_session
        .prompt(&mut leaky, &AgentInput::new("go"), now())
        .unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::SecretLeak(_)));

    // References (not values) flow through prompts untouched.
    let ok = session
        .prompt(
            &mut backend,
            &AgentInput::new("connect with ref")
                .with_secret_ref(SecretReference::new("secret://prod/db-password")),
            now(),
        )
        .unwrap();
    assert!(!ok.is_empty());
}

#[test]
fn sessions_interrupt_resume_and_time_out() {
    let mut backend = NativeAgent::new();
    let mut session = AgentSession::open(&backend, context(false), Vec::new()).unwrap();
    session
        .prompt(&mut backend, &AgentInput::new("start"), now())
        .unwrap();
    let len_before = session.events().len();

    // Interruption blocks prompts but keeps the log.
    session.interrupt().unwrap();
    assert_eq!(session.status(), SessionStatus::Interrupted);
    assert_eq!(
        session.events().last().map(|e| e.kind),
        Some(AgentEventKind::Interrupted)
    );
    assert!(session.interrupt().is_err(), "double interrupt must fail");
    let err = session
        .prompt(&mut backend, &AgentInput::new("while interrupted"), now())
        .unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::SessionState(_)));

    // Resume replays the full log so a new worker continues in order.
    let replayed = session.resume().unwrap();
    assert_eq!(replayed.len(), session.events().len());
    assert!(replayed.len() > len_before);
    assert_eq!(
        session.events().last().map(|e| e.kind),
        Some(AgentEventKind::Resumed)
    );
    assert!(session.resume().is_err(), "resume while active must fail");
    let continued = session
        .prompt(&mut backend, &AgentInput::new("after resume"), now())
        .unwrap();
    assert_eq!(
        continued.first().map(|e| e.sequence),
        Some((session.events().len() - continued.len()) as u64)
    );

    // Completion ends the session permanently.
    session.complete().unwrap();
    assert_eq!(session.status(), SessionStatus::Completed);
    assert!(session
        .prompt(&mut backend, &AgentInput::new("after complete"), now())
        .is_err());

    // A past deadline times out the session instead of running the prompt.
    let mut backend = NativeAgent::new();
    let mut expiring = AgentSession::open(&backend, context(false), Vec::new()).unwrap();
    expiring.set_deadline(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap());
    let err = expiring
        .prompt(&mut backend, &AgentInput::new("late"), now())
        .unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::Timeout(_)));
    assert_eq!(expiring.status(), SessionStatus::TimedOut);
    assert_eq!(
        expiring.events().last().map(|e| e.kind),
        Some(AgentEventKind::TimedOut)
    );
}

#[test]
fn platform_tool_precedence_prefers_platform_api() {
    assert_eq!(
        select_preferred_tool(&[ToolTier::RawShell, ToolTier::GenericTool]),
        Some(ToolTier::GenericTool)
    );
    assert_eq!(
        select_preferred_tool(&[
            ToolTier::RawShell,
            ToolTier::FrameworkTool,
            ToolTier::PlatformApi,
            ToolTier::GenericTool,
        ]),
        Some(ToolTier::PlatformApi)
    );
    assert_eq!(select_preferred_tool(&[]), None);
    // Policy allows non-shell tiers in production; denial is shell-specific.
    assert!(Policy::authorize(None, Some(ToolTier::GenericTool), true).is_ok());
    assert!(Policy::authorize(None, Some(ToolTier::RawShell), true).is_err());
    assert!(Policy::authorize(None, Some(ToolTier::RawShell), false).is_ok());
}

#[test]
fn external_adapter_normalizes_backend_specific_names() {
    let adapter = ExternalProcessAgent::new("opencode", AGENT_PROTOCOL_VERSION, Vec::new());
    // Backend-specific spellings land on the shared vocabulary.
    for (raw, expected) in [
        ("plan", AgentEventKind::Planning),
        ("opencode.plan", AgentEventKind::Planning),
        ("file-changed", AgentEventKind::FileChanged),
        ("exec", AgentEventKind::CommandRun),
        ("deploy", AgentEventKind::Deployment),
        ("verify", AgentEventKind::Verification),
        ("done", AgentEventKind::Completed),
    ] {
        let (kind, _) = adapter.normalize(raw, "detail".to_string()).unwrap();
        assert_eq!(kind, expected, "raw name '{raw}' must normalize");
    }
    // Unknown names are contract violations, never passed through.
    assert!(adapter.normalize("frobnicate", "x".to_string()).is_err());
    assert_eq!(adapter.command(), &[] as &[String]);
}

#[test]
fn native_reference_flow_runs_plan_to_verify() {
    let agent = NativeAgent::default();
    let phases = agent.reference_flow();
    assert_eq!(
        phases,
        [
            NativePhase::Plan,
            NativePhase::Act,
            NativePhase::Observe,
            NativePhase::Verify,
        ]
    );
    assert!(agent.identity().name.contains("native"));
    assert_eq!(
        agent.protocol_version(),
        AGENT_PROTOCOL_VERSION,
        "native backend speaks the current protocol"
    );
}
