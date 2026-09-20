use chrono::{Duration, TimeZone, Utc};

use labrys_core::{
    attach_secret_rows, authorize_dashboard_write, decode_response, encode_response,
    register_sdk_examples, render_dashboard, sdk_examples, Cli, CliActor, CliCommand, CliRequest,
    CliResponse, Correlation, DashboardSection, EventAction, EventActor, EventDraft, EventLog,
    EventResource, EventResult, ExampleAgentPlugin, ExampleDeployPlugin, Finding, HealthSnapshot,
    HealthState, LogLevel, LogSource, Plugin, PluginFamily, PluginManifest, PluginProcess,
    PluginRegistry, PluginState, RpcError, RpcErrorCode, RpcRequest, RpcResponse, SecretRow,
    Severity, VerifierStage, PLUGIN_PROTOCOL_VERSION,
};
use labrys_core::{ApplicationId, ExplicitApproval};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn human() -> CliActor {
    CliActor::human("bob")
}

fn agent_actor() -> CliActor {
    CliActor::agent("ses_1", "opencode")
}

fn platform_state() -> labrys_core::PlatformState {
    labrys_core::PlatformState::new()
}

// --- Requirement: CLI exposes the application lifecycle ---

#[test]
fn import_inspect_and_doctor_return_structured_findings() {
    let mut state = platform_state();
    let mut cli = Cli::new();

    // WHEN a user runs import, inspect, and doctor ...
    let import = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Import, human())
                .with_argument("path", ".")
                .with_idempotency_key("import-1"),
            &[],
            now(),
        )
        .unwrap();
    assert!(import.ok);
    let application_id: ApplicationId = import.application_id.expect("import registers one");
    assert_eq!(
        import
            .data
            .as_ref()
            .unwrap()
            .get("application_id")
            .unwrap()
            .as_str()
            .unwrap(),
        application_id.to_string()
    );

    let inspect = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Inspect, human()).on_application(application_id),
            &[],
            now(),
        )
        .unwrap();
    assert!(inspect.ok);
    assert_eq!(inspect.findings[0].severity, Severity::Info);

    let doctor = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Doctor, human()).on_application(application_id),
            &[],
            now(),
        )
        .unwrap();
    // THEN each returns findings tied to the application.
    assert_eq!(doctor.application_id, Some(application_id));
    assert!(doctor
        .findings
        .iter()
        .any(|f| f.code == "doctor.no_health_evidence"));
    assert!(doctor
        .findings
        .iter()
        .all(|f| f.recovery.is_some() || f.severity != Severity::Error));
}

#[test]
fn cli_failures_carry_actionable_recovery() {
    let mut state = platform_state();
    let mut cli = Cli::new();

    let unknown = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Inspect, human()).on_application(ApplicationId::new()),
            &[],
            now(),
        )
        .unwrap();
    assert!(!unknown.ok);
    assert!(unknown.has_errors());
    let finding = &unknown.findings[0];
    assert_eq!(finding.code, "cli.application_unknown");
    assert!(finding
        .recovery
        .as_deref()
        .unwrap()
        .contains("labrys import"));

    // A mutating command without an idempotency key is refused with a fix.
    let no_key = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Build, human())
                .on_application(ApplicationId::new())
                .in_environment("production"),
            &[],
            now(),
        )
        .unwrap();
    assert_eq!(no_key.findings[0].code, "cli.idempotency_required");
    assert!(no_key.findings[0]
        .recovery
        .as_deref()
        .unwrap()
        .contains("--idempotency-key"));
}

#[test]
fn cli_output_is_machine_readable() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let response = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Init, human())
                .with_argument("path", ".")
                .with_idempotency_key("init-1"),
            &[],
            now(),
        )
        .unwrap();
    let json = response.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in ["command", "ok", "replayed", "findings", "at"] {
        assert!(parsed.get(key).is_some(), "missing {key}");
    }
    assert_eq!(parsed["command"].as_str(), Some("init"));
    assert_eq!(parsed["ok"].as_bool(), Some(true));
    assert_eq!(parsed["findings"][0]["severity"].as_str(), Some("info"));
    assert_eq!(
        parsed["findings"][0]["code"].as_str(),
        Some("cli.registered")
    );
    assert_eq!(response.command, "init");
}

#[test]
fn idempotent_commands_replay_without_a_second_side_effect() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    let request = || {
        CliRequest::new(CliCommand::Deploy, human())
            .on_application(application)
            .in_environment("production")
            .with_argument("artifact", "sha256:aaa")
            .with_idempotency_key("deploy-1")
    };

    let first = cli.execute(&mut state, request(), &[], now()).unwrap();
    assert!(first.ok);
    assert!(!first.replayed);

    // A retry of the same key replays the recorded response.
    let second = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Deploy, human())
                .on_application(application)
                .in_environment("production")
                .with_argument("artifact", "sha256:bbb")
                .with_idempotency_key("deploy-1"),
            &[],
            now() + Duration::minutes(5),
        )
        .unwrap();
    assert!(second.replayed);
    assert_eq!(second.findings, first.findings);
    // The side effect happened once.
    assert_eq!(state.deployments[&application].len(), 1);
    assert!(state.deployments[&application][0].contains("sha256:aaa"));

    // A different key is a new invocation.
    let third = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Deploy, human())
                .on_application(application)
                .in_environment("production")
                .with_argument("artifact", "sha256:ccc")
                .with_idempotency_key("deploy-2"),
            &[],
            now(),
        )
        .unwrap();
    assert!(!third.replayed);
    assert_eq!(state.deployments[&application].len(), 2);
}

#[test]
fn agent_sessions_cannot_drive_lifecycle_mutations() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    for command in [
        CliCommand::Import,
        CliCommand::Build,
        CliCommand::Deploy,
        CliCommand::Rollback,
    ] {
        let denied = cli
            .execute(
                &mut state,
                CliRequest::new(command, agent_actor())
                    .on_application(application)
                    .in_environment("production")
                    .with_approval(ExplicitApproval::granted("bob", "ship")),
                &[],
                now(),
            )
            .unwrap();
        assert!(!denied.ok, "{command} must be denied to agents");
        assert_eq!(denied.findings[0].code, "cli.permission_denied");
    }

    // Read-only diagnosis stays available to the agent.
    let allowed = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Doctor, agent_actor()).on_application(application),
            &[],
            now(),
        )
        .unwrap();
    assert_eq!(allowed.findings[0].code, "doctor.no_health_evidence");
    assert!(!allowed.has_errors());
}

#[test]
fn rollback_requires_a_named_human_and_records_the_boundary() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    // No approval at all.
    let missing = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Rollback, human())
                .on_application(application)
                .in_environment("production")
                .with_idempotency_key("rb-0"),
            &[],
            now(),
        )
        .unwrap();
    assert_eq!(missing.findings[0].code, "cli.approval_required");

    // Denied approval.
    let denied = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Rollback, human())
                .on_application(application)
                .in_environment("production")
                .with_approval(ExplicitApproval::denied("bob", "rollback"))
                .with_idempotency_key("rb-1"),
            &[],
            now(),
        )
        .unwrap();
    assert_eq!(denied.findings[0].code, "cli.approval_required");

    // Granted, named approval succeeds and never claims data restoration.
    let ok = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Rollback, human())
                .on_application(application)
                .in_environment("production")
                .with_argument("target", "dep_previous")
                .with_approval(ExplicitApproval::granted("bob", "rollback"))
                .with_idempotency_key("rb-2"),
            &[],
            now(),
        )
        .unwrap();
    assert!(ok.ok);
    let data = ok.data.as_ref().unwrap();
    assert_eq!(data["database_data_restored"].as_bool(), Some(false));
    assert_eq!(data["approved_by"].as_str(), Some("bob"));
    assert!(data["warning"]
        .as_str()
        .unwrap()
        .contains("database data is not automatically rolled back"));
    assert_eq!(state.audit.len(), 1);
    state.audit.verify().unwrap();
}

#[test]
fn production_deploy_refuses_non_human_actors() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    let denied = cli
        .execute(
            &mut state,
            CliRequest::new(
                CliCommand::Deploy,
                CliActor::Platform {
                    name: "autoscaler".to_string(),
                },
            )
            .on_application(application)
            .in_environment("production")
            .with_idempotency_key("d-1"),
            &[],
            now(),
        )
        .unwrap();
    assert_eq!(denied.findings[0].code, "cli.permission_denied");
    assert!(state.deployments.is_empty());
}

#[test]
fn logs_and_health_commands_report_platform_evidence() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    state.logs.append(
        LogSource::Agent,
        LogLevel::Info,
        Correlation::new("t1").with_application(application),
        "agent edited src/auth.ts",
        &[],
        now(),
    );
    state.logs.append(
        LogSource::Deployment,
        LogLevel::Error,
        Correlation::new("t1").with_application(application),
        "image pull failed for password hunter2",
        &["hunter2"],
        now(),
    );
    state.health.insert(
        application,
        HealthSnapshot::new(
            "deployment-web",
            HealthState::Failed,
            now() - Duration::hours(2),
            Duration::minutes(5),
        )
        .with_detail("readiness probe answered 503"),
    );

    let logs = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Logs, human())
                .on_application(application)
                .with_argument("source", "deployment"),
            &[],
            now(),
        )
        .unwrap();
    assert!(logs.ok);
    let entries = logs.data.as_ref().unwrap()["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0]["message"].as_str().unwrap().contains("hunter2"));
    assert!(entries[0]["message"]
        .as_str()
        .unwrap()
        .contains(labrys_core::REDACTION_MARKER));

    // Stale evidence is a warning with recovery, never a green light.
    let health = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Health, human())
                .on_application(application)
                .in_environment("production"),
            &[],
            now(),
        )
        .unwrap();
    assert!(!health.ok);
    assert_eq!(health.findings[0].code, "cli.health");
    assert_eq!(health.findings[0].severity, Severity::Warning);
    assert!(health.findings[0]
        .recovery
        .as_deref()
        .unwrap()
        .contains("re-probe"));
    assert_eq!(health.data.as_ref().unwrap()["stale"].as_bool(), Some(true));
}

#[test]
fn doctor_aggregates_health_plugins_and_failed_evidence() {
    let mut state = platform_state();
    let mut cli = Cli::new();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);
    state.health.insert(
        application,
        HealthSnapshot::new("web", HealthState::Healthy, now(), Duration::minutes(5)),
    );
    // An incompatible plugin is registered as refused.
    let _ = state.plugins.register(PluginManifest::new(
        "old-inspector",
        "legacy-inspector",
        PluginFamily::InspectorProvider,
        PLUGIN_PROTOCOL_VERSION + 1,
        vec![
            "inspect.snapshot".to_string(),
            "inspect.findings".to_string(),
        ],
    ));
    let mut store = labrys_core::EvidenceStore::new(labrys_core::RetentionPolicy::default());
    store.append(labrys_core::VerificationEvidence::new(
        "ev-1",
        VerifierStage::Health,
        "auth endpoint",
        false,
        "callback returned 503",
        &[],
        now(),
    ));
    state.evidence = Some(store);

    let doctor = cli
        .execute(
            &mut state,
            CliRequest::new(CliCommand::Doctor, human()).on_application(application),
            &[],
            now(),
        )
        .unwrap();
    assert!(!doctor.ok);
    assert!(doctor
        .findings
        .iter()
        .any(|f| f.code == "doctor.plugin_unavailable" && f.recovery.is_some()));
    let evidence_finding = doctor
        .findings
        .iter()
        .find(|f| f.code == "doctor.failed_evidence")
        .unwrap();
    assert!(evidence_finding.message.contains("FAIL"));
    assert!(evidence_finding.recovery.is_some());
    assert!(doctor
        .findings
        .iter()
        .any(|f| f.code == "doctor.health" && f.severity == Severity::Info));
}

// --- Requirement: plugins use a process protocol ---

fn inspector_manifest(version: u32) -> PluginManifest {
    PluginManifest::new(
        "ts-inspector",
        "compose-inspector",
        PluginFamily::InspectorProvider,
        version,
        vec![
            "inspect.snapshot".to_string(),
            "inspect.findings".to_string(),
        ],
    )
    .with_command(vec!["node".to_string(), "inspector.js".to_string()])
    .with_capabilities(vec!["compose".to_string()])
}

#[test]
fn compatible_inspector_plugin_completes_the_handshake() {
    let mut registry = PluginRegistry::new();
    let family = registry
        .register(inspector_manifest(PLUGIN_PROTOCOL_VERSION))
        .unwrap();
    assert_eq!(family, PluginFamily::InspectorProvider);

    let plugin = registry
        .active_for(PluginFamily::InspectorProvider)
        .unwrap();
    // THEN the control plane can invoke it through the InspectorProvider
    // contract.
    assert_eq!(plugin.state(), PluginState::Active);
    let handshake = plugin.handshake().unwrap();
    assert_eq!(handshake.negotiated_version, PLUGIN_PROTOCOL_VERSION);
    assert_eq!(handshake.family, PluginFamily::InspectorProvider);
    assert!(handshake.capabilities.contains(&"compose".to_string()));
    assert_eq!(
        PluginFamily::InspectorProvider.required_methods(),
        &["inspect.snapshot", "inspect.findings"]
    );
}

#[test]
fn protocol_mismatch_refuses_activation_without_crashing() {
    let mut registry = PluginRegistry::new();
    let err = registry
        .register(inspector_manifest(PLUGIN_PROTOCOL_VERSION + 1))
        .unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::Plugin(_)));

    // The refused plugin stays registered for diagnosis ...
    let report = &registry.report()[0];
    assert_eq!(report.state, PluginState::Refused);
    assert!(report.detail.contains("version_mismatch"));
    assert!(registry
        .active_for(PluginFamily::InspectorProvider)
        .is_none());

    // ... and answers with a structured error instead of panicking.
    let mut plugin = PluginProcess::connect(inspector_manifest(PLUGIN_PROTOCOL_VERSION + 1));
    let refusal = plugin.refusal().unwrap();
    assert_eq!(refusal.code, RpcErrorCode::VersionMismatch);
    assert!(refusal.message.contains("offered protocol v2"));
    let response = plugin.invoke("inspect.findings", serde_json::json!({}), |_, _| {
        unreachable!("a refused plugin must never reach its handler")
    });
    assert!(!response.is_success());
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::VersionMismatch
    );
    assert!(response.error.as_ref().unwrap().recovery.is_some());
}

#[test]
fn incomplete_family_contract_is_refused() {
    let manifest = PluginManifest::new(
        "half-runtime",
        "half-runtime",
        PluginFamily::RuntimeProvider,
        PLUGIN_PROTOCOL_VERSION,
        vec!["runtime.detect".to_string()],
    );
    let plugin = PluginProcess::connect(manifest);
    assert_eq!(plugin.state(), PluginState::Refused);
    assert!(plugin
        .refusal()
        .unwrap()
        .message
        .contains("missing [\"runtime.prepare\"]"));
}

#[test]
fn unavailable_plugin_reports_structured_errors() {
    let mut plugin = PluginProcess::connect(inspector_manifest(PLUGIN_PROTOCOL_VERSION));
    let response = plugin.invoke(
        "inspect.snapshot",
        serde_json::json!({"files": []}),
        |_, _| Ok(serde_json::json!({ "files_seen": 0 })),
    );
    assert!(response.is_success());

    // The process dies.
    plugin.mark_unavailable();
    let response = plugin.invoke("inspect.snapshot", serde_json::json!({}), |_, _| {
        unreachable!("an unavailable plugin must not be invoked")
    });
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::PluginUnavailable
    );
    assert!(response.error.as_ref().unwrap().recovery.is_some());
    assert_eq!(plugin.call_count(), 1);

    // Shutdown is also explicit and non-panicking.
    let mut stopped = PluginProcess::connect(inspector_manifest(PLUGIN_PROTOCOL_VERSION));
    stopped.shutdown();
    assert_eq!(stopped.state(), PluginState::Stopped);
    let response = stopped.invoke("inspect.findings", serde_json::json!({}), |_, _| {
        unreachable!("a stopped plugin must not be invoked")
    });
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::PluginUnavailable
    );
}

#[test]
fn unknown_methods_and_cancellation_are_explicit() {
    let mut plugin = PluginProcess::connect(inspector_manifest(PLUGIN_PROTOCOL_VERSION));

    // Unknown method for the family.
    let response = plugin.invoke("inspect.deploy", serde_json::json!({}), |_, _| {
        unreachable!("unknown methods never reach the handler")
    });
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::MethodNotFound
    );

    // Cancelling a call that already finished is an error.
    let finished = plugin.invoke("inspect.snapshot", serde_json::json!({}), |_, _| {
        Ok(serde_json::json!({ "files_seen": 1 }))
    });
    assert!(plugin.cancel(finished.id).is_err());

    // A handler that fails answers internal_error, not a transport break.
    let response = plugin.invoke("inspect.findings", serde_json::json!({}), |_, _| {
        Err(labrys_core::CoreError::Plugin(
            "plugin panicked internally".to_string(),
        ))
    });
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::InvalidParams
    );
}

#[test]
fn cancelled_in_flight_call_reports_cancellation() {
    let mut plugin = PluginProcess::connect(inspector_manifest(PLUGIN_PROTOCOL_VERSION));
    // Simulate a call that is still in flight by cancelling the next id.
    let pending = plugin.next_pending_id();
    plugin.begin_call(pending, "inspect.snapshot");
    plugin.cancel(pending).unwrap();
    let response = plugin.cancelled_response(pending).unwrap();
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::Cancelled
    );
    assert!(response.error.as_ref().unwrap().recovery.is_some());
    assert_eq!(plugin.cancelled(), vec![pending]);
    assert!(plugin.cancelled_response(pending + 100).is_none());
}

#[test]
fn one_active_plugin_per_family() {
    let mut registry = PluginRegistry::new();
    registry
        .register(inspector_manifest(PLUGIN_PROTOCOL_VERSION))
        .unwrap();
    let err = registry
        .register(PluginManifest::new(
            "second-inspector",
            "other-inspector",
            PluginFamily::InspectorProvider,
            PLUGIN_PROTOCOL_VERSION,
            vec![
                "inspect.snapshot".to_string(),
                "inspect.findings".to_string(),
            ],
        ))
        .unwrap_err();
    assert!(err.to_string().contains("already has an active plugin"));
    // The rejected duplicate is never registered.
    assert_eq!(registry.len(), 1);
    assert!(registry
        .active_for(PluginFamily::InspectorProvider)
        .is_some());
}

#[test]
fn sdk_examples_cover_every_extension_family() {
    let mut registry = PluginRegistry::new();
    let families = register_sdk_examples(&mut registry).unwrap();
    assert_eq!(
        families,
        vec![
            PluginFamily::AgentProvider,
            PluginFamily::CapabilityProvider,
            PluginFamily::BindingProvider,
            PluginFamily::RuntimeProvider,
            PluginFamily::InspectorProvider,
            PluginFamily::DeployProvider,
        ]
    );
    for family in PluginFamily::all() {
        assert!(
            registry.active_for(*family).is_some(),
            "no active example for {family:?}"
        );
    }
    assert_eq!(sdk_examples().len(), 6);

    // Each example answers a real call through the contract.
    let mut agent = ExampleAgentPlugin::new("opencode");
    let response = agent.respond(&RpcRequest::new(
        1,
        "agent.draft_events",
        serde_json::json!({ "prompt": "add auth" }),
    ));
    assert!(response.is_success());
    assert_eq!(
        response.result.as_ref().unwrap()["events"][0]["kind"].as_str(),
        Some("planning")
    );

    let mut deploy = ExampleDeployPlugin;
    let response = deploy.respond(&RpcRequest::new(
        2,
        "deploy.rollback",
        serde_json::json!({ "digest": "sha256:old" }),
    ));
    let result = response.result.unwrap();
    assert_eq!(result["traffic_moved"].as_bool(), Some(true));
    assert_eq!(result["database_data_restored"].as_bool(), Some(false));

    // A method the plugin does not declare is refused before dispatch.
    let response = deploy.respond(&RpcRequest::new(3, "deploy.unknown", serde_json::json!({})));
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::MethodNotFound
    );

    // Missing required params surface as a plugin error.
    let response = deploy.respond(&RpcRequest::new(4, "deploy.build", serde_json::json!({})));
    assert_eq!(
        response.error.as_ref().unwrap().code,
        RpcErrorCode::InvalidParams
    );
}

#[test]
fn json_rpc_frames_round_trip() {
    let response = RpcResponse::ok(7, serde_json::json!({ "phase": "ready" }));
    let frame = encode_response(&response).unwrap();
    assert!(frame.contains("\"jsonrpc\":\"2.0\""));
    let back = decode_response(&frame).unwrap();
    assert_eq!(back, response);

    let failure = RpcResponse::err(
        8,
        RpcError::new(RpcErrorCode::Cancelled, "too slow").with_recovery("raise the timeout"),
    );
    let back = decode_response(&encode_response(&failure).unwrap()).unwrap();
    assert_eq!(back, failure);
    assert_eq!(back.error.as_ref().unwrap().code, RpcErrorCode::Cancelled);

    // Malformed and incomplete frames are errors, not panics.
    assert!(decode_response("not json").is_err());
    assert!(decode_response(r#"{"jsonrpc":"2.0","id":"x"}"#).is_err());
    assert!(decode_response(r#"{"jsonrpc":"2.0"}"#).is_err());
}

// --- Requirement: dashboard exposes ownership and health ---

#[test]
fn deployment_failure_after_agent_success_is_not_shown_as_healthy() {
    let mut state = platform_state();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);

    // The agent stream reports completion ...
    state.events = EventLog::new();
    state.events.append(
        EventDraft::new(
            now(),
            EventActor::agent(labrys_core::AgentSessionId::new(), "opencode"),
            Correlation::new("t9").with_application(application),
            EventAction::VerificationFinished,
            EventResult::Succeeded,
        ),
        &[],
    );
    // ... while platform evidence says the deployment is failing.
    state.logs.append(
        LogSource::Deployment,
        LogLevel::Error,
        Correlation::new("t9").with_application(application),
        "health gate failed: /healthz answered 503",
        &[],
        now(),
    );
    state.health.insert(
        application,
        HealthSnapshot::new("web", HealthState::Failed, now(), Duration::minutes(5)),
    );

    let view = render_dashboard(&state, application, "production", now()).unwrap();

    // THEN the dashboard shows the platform failure with its evidence source.
    assert!(!view.is_production_healthy());
    let failures = view.platform_failures();
    // Both the failed health probe and the failed deployment log are platform
    // failures; neither is hidden by the agent's completion claim.
    assert_eq!(failures.len(), 2);
    let log_failure = failures.iter().find(|f| f.key == "log1").unwrap();
    assert_eq!(
        log_failure.evidence_source.as_deref(),
        Some("platform:deployment-log")
    );
    assert_eq!(log_failure.state, Some(HealthState::Failed));
    assert!(failures
        .iter()
        .any(|f| f.evidence_source.as_deref() == Some("platform:health-probe")));

    // ... and the agent claim stays attributed to the agent, never merged into
    // platform health.
    let vibe = view.entries(DashboardSection::Vibe);
    assert_eq!(vibe.len(), 1);
    assert_eq!(vibe[0].attribution.kind_label(), "agent");
    assert!(vibe[0].state.is_none());
    assert!(view.agent_completion_claim.is_some());
    assert_eq!(
        view.entries(DashboardSection::Overview)[0]
            .attribution
            .kind_label(),
        "platform"
    );
}

#[test]
fn dashboard_reports_healthy_only_from_platform_evidence() {
    let mut state = platform_state();
    let application = ApplicationId::new();
    state.add_application(application, vec!["production".to_string()]);
    state.health.insert(
        application,
        HealthSnapshot::new("web", HealthState::Healthy, now(), Duration::minutes(5)),
    );
    state
        .capabilities
        .insert(application, vec!["database.postgres".to_string()]);
    state
        .resources
        .insert(application, vec!["res_db".to_string()]);
    state
        .deployments
        .insert(application, vec!["production:sha256:a".to_string()]);

    let mut view = render_dashboard(&state, application, "production", now()).unwrap();
    assert!(view.is_production_healthy());
    assert_eq!(view.entries(DashboardSection::Capabilities).len(), 1);
    assert_eq!(view.entries(DashboardSection::Resources).len(), 1);
    assert_eq!(view.entries(DashboardSection::Deployments).len(), 1);

    // Secrets carry references only, never values.
    attach_secret_rows(
        &mut view,
        vec![SecretRow {
            ref_id: "neon.database_url".to_string(),
            version_hint: "v3".to_string(),
            environment: "production".to_string(),
        }],
    );
    let rendered = serde_json::to_string(&view).unwrap();
    assert!(rendered.contains("neon.database_url"));
    assert!(!rendered.contains("postgres://"));
    assert!(view.secrets.len() == 1);

    // A stale healthy snapshot is not healthy.
    let stale = HealthSnapshot::new(
        "web",
        HealthState::Healthy,
        now() - Duration::hours(1),
        Duration::minutes(5),
    );
    state.health.insert(application, stale);
    let view = render_dashboard(&state, application, "production", now()).unwrap();
    assert!(!view.is_production_healthy());
}

#[test]
fn dashboard_write_boundaries_are_enforced() {
    // Read-only sections never accept writes.
    for section in [
        DashboardSection::Secrets,
        DashboardSection::Logs,
        DashboardSection::CodeChanges,
    ] {
        let err = authorize_dashboard_write(
            section,
            &human(),
            Some(&ExplicitApproval::granted("bob", "x")),
        )
        .unwrap_err();
        assert!(matches!(err, labrys_core::CoreError::PermissionDenied(_)));
    }

    // Agents cannot write through the dashboard at all.
    let err = authorize_dashboard_write(DashboardSection::Apps, &agent_actor(), None).unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::PermissionDenied(_)));

    // Approval-gated sections need a granted, named approval.
    for approval in [None, Some(ExplicitApproval::denied("bob", "x"))] {
        assert!(authorize_dashboard_write(
            DashboardSection::Deployments,
            &human(),
            approval.as_ref()
        )
        .is_err());
    }
    let anonymous = ExplicitApproval {
        approved: true,
        approver: " ".to_string(),
        proposal_summary: "x".to_string(),
    };
    assert!(
        authorize_dashboard_write(DashboardSection::Settings, &human(), Some(&anonymous)).is_err()
    );
    authorize_dashboard_write(
        DashboardSection::Deployments,
        &human(),
        Some(&ExplicitApproval::granted("bob", "deploy")),
    )
    .unwrap();

    // Non-gated sections accept a human write.
    authorize_dashboard_write(DashboardSection::Apps, &human(), None).unwrap();
    assert_eq!(DashboardSection::all().len(), 12);
    assert_eq!(
        DashboardSection::CodeChanges.canonical_name(),
        "code_changes"
    );
}

#[test]
fn cli_covers_the_stable_command_vocabulary() {
    let names: Vec<&str> = CliCommand::all()
        .iter()
        .map(|c| c.canonical_name())
        .collect();
    for required in [
        "init",
        "import",
        "inspect",
        "dev",
        "agent",
        "preview",
        "capability",
        "resources",
        "build",
        "deploy",
        "deployments",
        "logs",
        "health",
        "rollback",
        "doctor",
    ] {
        assert!(names.contains(&required), "missing command {required}");
    }
    assert!(CliCommand::Rollback.is_mutating());
    assert!(!CliCommand::Inspect.is_mutating());
    assert!(CliCommand::Deploy.requires_environment());
    assert!(!CliCommand::Init.requires_application());
    assert_eq!(CliCommand::Doctor.to_string(), "doctor");
}

#[test]
fn render_dashboard_rejects_unknown_applications() {
    let state = platform_state();
    let err = render_dashboard(&state, ApplicationId::new(), "production", now()).unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::NotFound(_)));
}

#[test]
fn findings_render_as_json_and_carry_resources() {
    let finding = Finding::error("doctor.failed_evidence", "auth callback 503")
        .with_recovery("re-register the provider callback")
        .on_resource(EventResource::new("auth", "google-oauth"));
    let value = finding.to_json_value();
    assert_eq!(value["severity"].as_str(), Some("error"));
    assert_eq!(value["resource"]["kind"].as_str(), Some("auth"));
    assert!(serde_json::to_string(&finding)
        .unwrap()
        .contains("re-register"));
    let response: CliResponse = CliResponse {
        command: "doctor".to_string(),
        ok: false,
        replayed: false,
        application_id: Some(ApplicationId::new()),
        environment: None,
        findings: vec![finding],
        data: None,
        idempotency_key: None,
        at: now(),
    };
    assert!(response.has_errors());
    assert!(response.to_event_detail().contains("FAILED"));
}
