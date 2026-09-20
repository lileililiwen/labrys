use chrono::{Duration, TimeZone, Utc};

use labrys_core::AgentSessionId;
use labrys_core::{
    ApplicationId, AuditLog, Correlation, EnvironmentId, EventAction, EventActor, EventDraft,
    EventLog, EventResource, EventResult, EvidenceStore, HealthSnapshot, HealthState, LogSource,
    LogStore, RetentionPolicy, StageResult, UsageLedger, UsageRecord, VerificationEvidence,
    VerificationVerdict, Verifier, VerifierStage, OBSERVABILITY_CONTRACT_VERSION,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn correlation() -> Correlation {
    Correlation::new("trace-1")
        .with_session(AgentSessionId::new())
        .with_application(ApplicationId::new())
        .with_environment(EnvironmentId::new())
}

fn passing(stage: VerifierStage, id: &str, check: &str) -> StageResult {
    StageResult::passed(
        stage,
        vec![VerificationEvidence::new(
            id,
            stage,
            check,
            true,
            "observed expected state",
            &[],
            now(),
        )],
    )
}

fn failing(
    stage: VerifierStage,
    id: &str,
    check: &str,
    summary: &str,
    secrets: &[&str],
) -> StageResult {
    StageResult::failed(
        stage,
        vec![
            VerificationEvidence::new(id, stage, check, false, summary, secrets, now())
                .on_resource(EventResource::new("auth", "google-oauth"))
                .with_recovery("re-run the provider callback registration and re-probe"),
        ],
    )
}

fn all_blocking_pass() -> Verifier {
    let mut verifier = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Health,
        VerifierStage::Schema,
        VerifierStage::Browser,
        VerifierStage::Human,
    ] {
        verifier
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    verifier
}

// --- Requirement: platform actions are attributable ---

#[test]
fn capability_requests_are_attributed_to_the_agent_and_platform_decision() {
    let mut log = EventLog::new();
    let correlation = correlation();
    let session = correlation.session_id.unwrap();
    let application = correlation.application_id.unwrap();
    let environment = correlation.environment_id.unwrap();

    // WHEN an agent requests a capability and the platform decides ...
    log.append(
        EventDraft::new(
            now(),
            EventActor::agent(session, "cursor"),
            correlation.clone(),
            EventAction::CapabilityRequested,
            EventResult::Accepted,
        )
        .on_resource(EventResource::new("auth", "auth0")),
        &[],
    );
    log.append(
        EventDraft::new(
            now() + Duration::seconds(1),
            EventActor::platform("capability-controller"),
            correlation.clone(),
            EventAction::CapabilityDecision,
            EventResult::Denied,
        )
        .on_resource(EventResource::new("auth", "auth0")),
        &[],
    );

    // THEN the events identify the agent session and the platform decision ...
    let session_events = log.for_session(session);
    assert_eq!(session_events.len(), 2);
    assert_eq!(session_events[0].actor.kind_label(), "agent");
    assert_eq!(session_events[1].actor.kind_label(), "platform");
    assert_eq!(session_events[1].result, EventResult::Denied);

    // ... AND the application and environment are queryable.
    assert_eq!(log.for_application(application).len(), 2);
    assert_eq!(log.for_environment(environment).len(), 2);
    assert_eq!(log.for_trace("trace-1").len(), 2);
    assert_eq!(log.events().first().unwrap().sequence, 1);
    assert_eq!(log.events().last().unwrap().sequence, 2);
}

#[test]
fn events_carry_before_and_after_state() {
    let mut log = EventLog::new();
    log.append(
        EventDraft::new(
            now(),
            EventActor::human("bob"),
            Correlation::new("trace-2"),
            EventAction::SecretRotated,
            EventResult::Succeeded,
        )
        .on_resource(EventResource::new("secret", "db.url"))
        .between("v1", "v2"),
        &[],
    );
    let event = log.events().first().unwrap();
    assert_eq!(event.before.as_deref(), Some("v1"));
    assert_eq!(event.after.as_deref(), Some("v2"));
    assert!(event.to_event_detail().contains("secret_rotated"));
}

#[test]
fn event_free_text_is_redacted_before_it_is_persisted() {
    let mut log = EventLog::new();
    let secrets = ["hunter2"];
    log.append(
        EventDraft::new(
            now(),
            EventActor::platform("resource-controller"),
            Correlation::new("trace-3"),
            EventAction::ResourceProvisioned,
            EventResult::Failed {
                reason: "auth failed with password hunter2".to_string(),
            },
        )
        .between("config password=hunter2", "no state"),
        &secrets,
    );
    let event = log.events().first().unwrap();
    let rendered = format!("{event:?}{}", event.to_event_detail());
    assert!(!rendered.contains("hunter2"));
    assert!(rendered.contains(labrys_core::REDACTION_MARKER));
    assert!(event.result.is_failure());
}

// --- Logs separated by source ---

#[test]
fn log_streams_stay_separated_by_source() {
    let mut store = LogStore::new();
    let correlation = Correlation::new("trace-4");
    for source in [
        LogSource::Agent,
        LogSource::Build,
        LogSource::Runtime,
        LogSource::Deployment,
        LogSource::Capability,
        LogSource::Resource,
    ] {
        store.append(
            source,
            labrys_core::LogLevel::Info,
            correlation.clone(),
            &format!("{source:?} line"),
            &[],
            now(),
        );
    }
    store.append(
        LogSource::Resource,
        labrys_core::LogLevel::Error,
        correlation.clone(),
        "provider returned credential password=hunter2 for db.internal",
        &["hunter2"],
        now(),
    );

    // Each stream holds only its own lines.
    assert_eq!(store.for_source(LogSource::Agent).len(), 1);
    assert_eq!(store.for_source(LogSource::Resource).len(), 2);
    assert_eq!(store.entries().len(), 7);

    // A noisy agent stream cannot bury the resource failure.
    let errors = store.errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].source, LogSource::Resource);

    // AND the persisted diagnostic carries no credential material while still
    // naming the failing resource.
    assert!(!errors[0].message.contains("hunter2"));
    assert!(errors[0].message.contains("db.internal"));
    assert!(errors[0].message.contains(labrys_core::REDACTION_MARKER));
}

// --- Health evidence and staleness ---

#[test]
fn stale_health_evidence_never_supports_a_readiness_claim() {
    let snapshot = HealthSnapshot::new(
        "deployment-web",
        HealthState::Healthy,
        now(),
        Duration::minutes(5),
    );
    assert!(!snapshot.is_stale(now()));
    assert_eq!(
        snapshot.effective_state(now() + Duration::minutes(1)),
        HealthState::Healthy
    );

    // Past the ttl the observation is no longer evidence of anything.
    let later = now() + Duration::minutes(6);
    assert!(snapshot.is_stale(later));
    assert_eq!(snapshot.effective_state(later), HealthState::Unknown);
}

#[test]
fn stale_health_check_fails_the_health_stage() {
    let snapshot = HealthSnapshot::new("auth", HealthState::Healthy, now(), Duration::minutes(5));
    let stale_at = now() + Duration::minutes(10);
    let effective = snapshot.effective_state(stale_at);

    let mut verifier = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Schema,
        VerifierStage::Browser,
        VerifierStage::Human,
    ] {
        verifier
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    verifier
        .add_stage(StageResult::passed(
            VerifierStage::Health,
            vec![VerificationEvidence::new(
                "health",
                VerifierStage::Health,
                "auth endpoint probe",
                effective == HealthState::Healthy,
                &format!("probe state {effective:?} at {stale_at}"),
                &[],
                stale_at,
            )
            .on_resource(EventResource::new("auth", "google-oauth"))
            .with_recovery("re-probe the auth endpoint after refreshing the token")],
        ))
        .unwrap();

    // Stale evidence cannot be outvoted by the other passing stages.
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Failed {
            first_failure: VerifierStage::Health
        }
    );
    let explanation = verifier.failures().remove(0);
    assert_eq!(explanation.resource.as_deref(), Some("google-oauth (auth)"));
    assert!(explanation.recovery.contains("re-probe"));
}

// --- Audit immutability ---

#[test]
fn audit_chain_detects_tampering() {
    let mut audit = AuditLog::new();
    audit.append(now(), "bob", "secret.replace", "db.url", "v1 -> v2", &[]);
    audit.append(
        now(),
        "alice",
        "domain.attach",
        "app.example.com",
        "routed",
        &[],
    );
    audit.append(
        now(),
        "bob",
        "deployment.rollback",
        "dep_1",
        "image only",
        &[],
    );
    assert_eq!(audit.len(), 3);
    audit.verify().unwrap();

    // Chained: each record links to its predecessor.
    let records = audit.records();
    assert_eq!(records[0].prev_hash, AuditLog::GENESIS);
    assert_eq!(records[1].prev_hash, records[0].hash);
    assert_eq!(records[2].prev_hash, records[1].hash);

    // Editing a persisted record breaks the chain.
    let mut tampered: Vec<labrys_core::AuditRecord> = records.to_vec();
    tampered[1].detail = "rewritten history".to_string();
    let tampered = AuditLog::restore(tampered);
    let err = tampered.verify().unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::AuditIntegrity(_)));
    assert!(format!("{err}").contains("digest mismatch at record 2"));

    // Dropping a record breaks the sequence and the link.
    let mut truncated: Vec<labrys_core::AuditRecord> = records.to_vec();
    truncated.remove(0);
    assert!(AuditLog::restore(truncated).verify().is_err());
}

#[test]
fn audit_details_are_redacted_at_append() {
    let mut audit = AuditLog::new();
    audit.append(
        now(),
        "provider",
        "resource.provision",
        "db.internal",
        "auth failed for password hunter2",
        &["hunter2"],
    );
    let record = audit.records().first().unwrap();
    assert!(!record.detail.contains("hunter2"));
    assert!(record.detail.contains(labrys_core::REDACTION_MARKER));
    // The failing resource stays identifiable.
    assert_eq!(record.target, "db.internal");
    assert!(audit.verify().is_ok());
    assert!(audit.to_event_detail().contains("resource.provision"));
}

// --- Usage accounting ---

#[test]
fn usage_ledger_aggregates_per_application() {
    let application = ApplicationId::new();
    let other = ApplicationId::new();
    let environment = EnvironmentId::new();
    let mut ledger = UsageLedger::new();
    for (app, cpu, requests) in [
        (application, 1000, 10),
        (application, 500, 7),
        (other, 9, 1),
    ] {
        ledger.record(UsageRecord {
            application_id: app,
            environment_id: environment,
            period_start: now(),
            period_end: now() + Duration::hours(1),
            cpu_millicore_seconds: cpu,
            memory_mb_seconds: cpu * 2,
            build_seconds: 30,
            requests,
            egress_bytes: 1024,
        });
    }
    let totals = ledger.totals(application);
    assert_eq!(totals.cpu_millicore_seconds, 1500);
    assert_eq!(totals.requests, 17);
    assert_eq!(totals.egress_bytes, 2048);
    assert_eq!(ledger.for_application(other).len(), 1);
}

// --- Requirement: completion requires independent evidence ---

#[test]
fn agent_completion_text_never_produces_a_successful_verdict() {
    let mut verifier = Verifier::new();

    // WHEN the agent reports completion ...
    verifier.record_agent_claim("Google login is done, ship it");

    // ... with nothing independently checked, completion is not available.
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Incomplete {
            missing: vec![
                VerifierStage::Deterministic,
                VerifierStage::BuildTest,
                VerifierStage::Health,
                VerifierStage::Schema,
                VerifierStage::Browser,
                VerifierStage::Human,
            ]
        }
    );

    // THEN verification checks capability readiness, callback reachability,
    // build state, and auth endpoint health independently of the claim.
    let mut verifier = Verifier::new();
    verifier.record_agent_claim("Google login is done, ship it");
    verifier
        .add_stage(passing(
            VerifierStage::Deterministic,
            "det",
            "contract version",
        ))
        .unwrap();
    verifier
        .add_stage(passing(VerifierStage::BuildTest, "build", "cargo test"))
        .unwrap();
    verifier
        .add_stage(failing(
            VerifierStage::Health,
            "auth-health",
            "auth endpoint health",
            "GET /auth/callback answered 503",
            &[],
        ))
        .unwrap();
    verifier
        .add_stage(failing(
            VerifierStage::Browser,
            "callback",
            "callback reachability",
            "login redirect never reached the app",
            &[],
        ))
        .unwrap();
    verifier
        .add_stage(StageResult::not_applicable(VerifierStage::Schema))
        .unwrap();
    verifier
        .add_stage(passing(VerifierStage::Human, "human", "operator confirmed"))
        .unwrap();

    // AND a failed check prevents a successful completion result.
    let verdict = verifier.verdict();
    assert!(!verdict.is_success());
    assert_eq!(
        verdict,
        VerificationVerdict::Failed {
            first_failure: VerifierStage::Health
        }
    );
    assert_eq!(
        verifier.agent_claim(),
        Some("Google login is done, ship it")
    );

    // Only when every applicable blocking stage passes does it succeed.
    let mut fixed = all_blocking_pass();
    fixed
        .add_stage(StageResult::not_applicable(VerifierStage::Schema))
        .unwrap_err(); // schema already recorded by the helper
    assert!(all_blocking_pass().verdict().is_success());
}

#[test]
fn applicable_stage_without_evidence_is_incomplete() {
    let mut verifier = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Schema,
        VerifierStage::Browser,
        VerifierStage::Human,
    ] {
        verifier
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    // Health applies but produced no evidence at all.
    verifier
        .add_stage(StageResult::passed(VerifierStage::Health, Vec::new()))
        .unwrap();
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Incomplete {
            missing: vec![VerifierStage::Health]
        }
    );

    // A not-applicable stage is never listed as missing.
    let mut no_browser = all_blocking_pass();
    no_browser
        .add_stage(StageResult::not_applicable(VerifierStage::Browser))
        .unwrap_err();
    let mut verifier = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Health,
        VerifierStage::Schema,
        VerifierStage::Human,
    ] {
        verifier
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    verifier
        .add_stage(StageResult::not_applicable(VerifierStage::Browser))
        .unwrap();
    assert!(verifier.missing_evidence().is_empty());
    assert!(verifier.verdict().is_success());
}

#[test]
fn later_stages_cannot_erase_earlier_failures() {
    let mut verifier = Verifier::new();
    verifier
        .add_stage(failing(
            VerifierStage::BuildTest,
            "build-1",
            "cargo test",
            "3 tests failed",
            &[],
        ))
        .unwrap();

    // A later "pass" for the same stage is recorded as disagreement only.
    verifier
        .add_stage(passing(VerifierStage::BuildTest, "build-2", "cargo test"))
        .unwrap();
    assert_eq!(verifier.disagreements().len(), 1);
    assert!(verifier.disagreements()[0].contains("already failed"));
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Failed {
            first_failure: VerifierStage::BuildTest
        }
    );
    // The original failure evidence is what remains on record.
    assert_eq!(
        verifier
            .stage(VerifierStage::BuildTest)
            .unwrap()
            .evidence
            .len(),
        1
    );
    assert!(!verifier.stage(VerifierStage::BuildTest).unwrap().evidence[0].passed);

    // Re-recording a non-conflicting stage is a contract violation.
    assert!(verifier
        .add_stage(passing(VerifierStage::Health, "h", "probe"))
        .is_ok());
    assert!(verifier
        .add_stage(passing(VerifierStage::Health, "h2", "probe"))
        .is_err());
}

#[test]
fn advisory_llm_opinion_never_blocks_or_rescues_completion() {
    let mut verifier = all_blocking_pass();
    // The advisory model disagrees, but advisory is non-blocking.
    verifier
        .add_stage(failing(
            VerifierStage::Advisory,
            "llm",
            "llm review",
            "advice: the diff looks risky",
            &[],
        ))
        .unwrap();
    assert!(verifier.verdict().is_success());
    assert!(!verifier
        .missing_evidence()
        .contains(&VerifierStage::Advisory));
    // Advisory evidence is still retained for diagnosis.
    assert_eq!(
        verifier.for_stage_evidence(VerifierStage::Advisory).len(),
        1
    );

    // Advisory praise cannot rescue a failed blocking stage.
    let mut risky = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Health,
        VerifierStage::Schema,
        VerifierStage::Browser,
        VerifierStage::Human,
    ] {
        risky
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    risky
        .add_stage(failing(
            VerifierStage::Browser,
            "browser",
            "checkout flow",
            "button never enabled",
            &[],
        ))
        .unwrap();
    risky
        .add_stage(passing(VerifierStage::Advisory, "llm", "llm review"))
        .unwrap();
    assert!(!risky.verdict().is_success());
}

#[test]
fn human_verification_is_a_blocking_stage() {
    let mut verifier = Verifier::new();
    for stage in [
        VerifierStage::Deterministic,
        VerifierStage::BuildTest,
        VerifierStage::Health,
        VerifierStage::Schema,
        VerifierStage::Browser,
    ] {
        verifier
            .add_stage(passing(stage, stage.canonical_name(), "check"))
            .unwrap();
    }
    // Without the human stage the verdict stays incomplete.
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Incomplete {
            missing: vec![VerifierStage::Human]
        }
    );
    // A human refusing makes it fail.
    verifier
        .add_stage(failing(
            VerifierStage::Human,
            "human",
            "operator review",
            "operator rejected the rollout",
            &[],
        ))
        .unwrap();
    assert_eq!(
        verifier.verdict(),
        VerificationVerdict::Failed {
            first_failure: VerifierStage::Human
        }
    );
}

// --- Requirement: secret-bearing evidence is redacted ---

#[test]
fn provider_credential_errors_persist_as_redacted_diagnostics() {
    let secrets = ["sk-live-9f2e", "hunter2"];
    let stage = failing(
        VerifierStage::Health,
        "ev-1",
        "provider readiness",
        "postgres rejected password hunter2 with token sk-live-9f2e on db.internal:5432",
        &secrets,
    );
    let evidence = &stage.evidence[0];
    let rendered = format!("{evidence:?}{}", evidence.to_event_detail());

    // THEN persisted evidence holds no credential material ...
    assert!(!rendered.contains("hunter2"));
    assert!(!rendered.contains("sk-live-9f2e"));
    assert!(evidence.summary.contains(labrys_core::REDACTION_MARKER));

    // ... AND operators can still identify the failing resource and the
    // recovery path.
    let explanation = evidence.explanation();
    assert_eq!(explanation.resource.as_deref(), Some("google-oauth (auth)"));
    assert!(explanation
        .recovery
        .contains("provider callback registration"));
    assert!(explanation.to_event_detail().contains("health"));
    assert!(!explanation.to_event_detail().contains("hunter2"));
    assert!(!evidence.passed);
}

#[test]
fn evidence_store_separates_and_applies_retention() {
    let policy = RetentionPolicy {
        max_age: Duration::days(30),
        keep_latest: 2,
    };
    let mut store = EvidenceStore::new(policy);

    // Old passing evidence beyond the keep window is pruned.
    for i in 0..5 {
        store.append(VerificationEvidence::new(
            format!("old-{i}"),
            VerifierStage::BuildTest,
            "cargo test",
            true,
            "passed",
            &[],
            now() - Duration::days(60),
        ));
    }
    store.append(VerificationEvidence::new(
        "recent",
        VerifierStage::BuildTest,
        "cargo test",
        true,
        "passed",
        &[],
        now(),
    ));
    let removed = store.retain(now());
    assert_eq!(removed.len(), 4);
    assert_eq!(store.evidence().len(), 2);
    assert!(store.evidence().iter().all(|e| e.id != "old-0"));
    assert_eq!(store.for_stage(VerifierStage::BuildTest).len(), 2);

    // An unresolved failure is retained whatever its age.
    let mut store = EvidenceStore::new(policy);
    store.append(VerificationEvidence::new(
        "ancient-failure",
        VerifierStage::Schema,
        "migration state",
        false,
        "migration 42 not applied",
        &[],
        now() - Duration::days(400),
    ));
    store.append(VerificationEvidence::new(
        "ancient-pass",
        VerifierStage::Schema,
        "schema drift",
        true,
        "no drift",
        &[],
        now() - Duration::days(400),
    ));
    let removed = store.retain(now());
    assert_eq!(removed, vec!["ancient-pass".to_string()]);
    assert_eq!(store.evidence().len(), 1);
    assert_eq!(store.evidence()[0].id, "ancient-failure");
    assert_eq!(store.policy(), policy);
}

#[test]
fn evidence_and_snapshots_round_trip_through_json() {
    let evidence = VerificationEvidence::new(
        "ev-9",
        VerifierStage::Browser,
        "checkout flow",
        true,
        "all steps completed",
        &[],
        now(),
    )
    .on_resource(EventResource::new("deployment", "dep_1"));
    let json = serde_json::to_string(&evidence).unwrap();
    let back: VerificationEvidence = serde_json::from_str(&json).unwrap();
    assert_eq!(back, evidence);

    let snapshot = HealthSnapshot::new("web", HealthState::Degraded, now(), Duration::minutes(5))
        .with_detail("one replica lagging");
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: HealthSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(back, snapshot);

    let audit = AuditLog::new();
    let json = serde_json::to_string(&audit).unwrap();
    let back: AuditLog = serde_json::from_str(&json).unwrap();
    assert_eq!(back, audit);
    assert_eq!(OBSERVABILITY_CONTRACT_VERSION, 1);
}
