//! PostgreSQL-backed integration tests for the durable control plane.
//!
//! These exercise real migrations, transactions, leases, and concurrency against
//! an isolated database. When no database URL is configured the whole file
//! skips (the pure-core suite still proves the deterministic models). They are
//! infrastructure evidence, not production-deployment evidence.

use std::sync::OnceLock;

use chrono::{Duration, Utc};
use sqlx::{PgPool, Row};
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    connect, run_migrations, Config, ControlPlaneError, DispatchOutcome, NoopDispatcher,
    PgApplicationStore, PgAuditLog, PgEventStore, PgEvidenceStore, PgJobQueue, PgLogStore,
    PgResourceStore, PgUsageLedger, ScriptedDispatcher, Worker,
};
use labrys_core::environment::EnvironmentRefs;
use labrys_core::{
    Application, ApplicationId, Correlation, EventAction, EventActor, EventDraft, EventResource,
    EventResult, IdempotencyKey, JobAction, JobStatus, ObservationSource, ObservedState,
    ObservedStatus, Origin, Resource, ResourcePhase, RetentionPolicy, RetryPolicy, UsageRecord,
    VerificationEvidence, VerifierStage,
};

const SECRET: &str = "hunter2-db-password-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

/// Serializes integration tests on the shared database and gives each one a
/// clean, migrated schema. Returns `None` (skip) when no database is configured.
async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let url = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    let guard = lock.lock().await;
    let pool = PgPool::connect(&url).await.expect("connect postgres");
    run_migrations(&pool).await.expect("run migrations");
    sqlx::query(
        "TRUNCATE applications, environments, desired_states, observed_states, resources, \
         deployments, capabilities, events, logs, audit_log, evidence, usage_records, jobs \
         RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    Some((guard, pool))
}

fn sample_application() -> Application {
    Application::new(
        "svc",
        Origin::Generated {
            prompt_summary: None,
        },
        "tester",
    )
}

fn test_config(worker_id: &str) -> Config {
    Config {
        database_url: database_url().expect("db url"),
        worker_id: worker_id.to_string(),
        lease_seconds: 60,
        poll_interval_ms: 10,
        drain_timeout_ms: 200,
        max_concurrency: 1,
        run_migrations: false,
        runtime_mode: labrys_control_plane::RuntimeMode::Disabled,
        docker_bin: std::path::PathBuf::from("docker"),
        workspace_root: std::env::temp_dir().join("labrys-test-workspaces"),
        preview_ttl_secs: 3_600,
    }
}

// ---------------------------------------------------------------------------
// Authoritative state survives process boundaries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restart_preserves_desired_and_observed_state() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let store = PgApplicationStore::new(pool.clone());
    let mut app = sample_application();
    let desired = app
        .desired_state
        .with_config(serde_json::json!({"replicas": 3}));
    app.update_desired_state(desired, "tester");
    app.record_observation(ObservedState {
        desired_version: app.desired_state.version,
        status: ObservedStatus::Healthy,
        last_observed_at: Utc::now(),
        detail: Some("ok".to_string()),
    });
    store.save(&app).await.unwrap();

    // A brand-new pool and store stand in for a restarted process.
    let fresh = PgApplicationStore::new(connect(&test_config("restart")).await.unwrap());
    let loaded = fresh.get(&app.id).await.unwrap();
    assert_eq!(loaded.desired_state.version, 2);
    assert_eq!(loaded.desired_state.config["replicas"], 3);
    assert_eq!(loaded.observed_state.status, ObservedStatus::Healthy);
    assert_eq!(loaded.environments.len(), 2);
}

#[tokio::test]
async fn stale_generation_cannot_overwrite_newer_state() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let store = PgApplicationStore::new(pool.clone());
    let app = sample_application();
    store.save(&app).await.unwrap();

    let next = app.desired_state.with_config(serde_json::json!({"v": 2}));
    let event = reconcile_event(&app.id);
    store
        .update_desired_state(&app.id, 1, &next, &event, "v2", "tester", &[])
        .await
        .unwrap();

    // A second writer still holding generation 1 must conflict.
    let stale = app
        .desired_state
        .with_config(serde_json::json!({"v": "stale"}));
    let err = store
        .update_desired_state(&app.id, 1, &stale, &event, "stale", "other", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Conflict(_)), "{err:?}");

    let stored = store.get(&app.id).await.unwrap();
    assert_eq!(stored.desired_state.version, 2);
    assert_eq!(stored.desired_state.config["v"], 2);
}

#[tokio::test]
async fn preview_write_cannot_change_production_state() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let store = PgApplicationStore::new(pool.clone());
    let mut app = sample_application();
    app.add_preview_environment("pr-42");
    store.save(&app).await.unwrap();

    let preview = app.environment_by_name("pr-42").unwrap().clone();
    let production = app.environment_by_name("production").unwrap().clone();

    let mut refs = EnvironmentRefs::default();
    refs.runtime
        .insert("preview".to_string(), preview.id.to_string());
    store
        .set_environment_refs(&app.id, &preview.id, &refs)
        .await
        .unwrap();

    let after = store.get(&app.id).await.unwrap();
    let prod_after = after.environment_by_name("production").unwrap();
    assert!(prod_after.refs.runtime.is_empty());
    assert_eq!(
        after.environment_by_name("pr-42").unwrap().refs.runtime["preview"],
        preview.id.to_string()
    );

    // A foreign application's environment id is rejected (isolation).
    let mut other = sample_application();
    other.name = "other".into();
    store.save(&other).await.unwrap();
    let other_env = other.environment_by_name("production").unwrap().clone();
    let err = store
        .set_environment_refs(&app.id, &other_env.id, &EnvironmentRefs::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ControlPlaneError::EnvironmentIsolation(_)),
        "{err:?}"
    );
    let _ = production;
}

// ---------------------------------------------------------------------------
// Persisted evidence is attributable and redacted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn credential_bearing_provider_failure_is_stored_redacted() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let app = sample_application();
    PgApplicationStore::new(pool.clone())
        .save(&app)
        .await
        .unwrap();

    let events = PgEventStore::new(pool.clone());
    let draft = EventDraft::new(
        Utc::now(),
        EventActor::Provider {
            name: "postgres".to_string(),
        },
        Correlation::new("trace-1").with_application(app.id),
        EventAction::ResourceProvisioned,
        EventResult::Failed {
            reason: format!("auth failed with {SECRET}"),
        },
    )
    .on_resource(EventResource::new("database.postgres", "primary"))
    .between(format!("dsn with {SECRET}"), "none");
    let stored = events.append(&draft, &[SECRET]).await.unwrap();
    assert!(!stored.to_event_detail().contains(SECRET));
    assert!(stored.to_event_detail().contains("[redacted]"));

    // No persisted event field leaks the secret.
    let all = events.for_application(&app.id.to_string()).await.unwrap();
    for event in &all {
        assert!(!format!("{event:?}").contains(SECRET));
    }

    // Audit and log surfaces are redacted too.
    let audit = PgAuditLog::new(pool.clone());
    audit
        .append(
            Utc::now(),
            "worker-1",
            "provision",
            "primary",
            &format!("provider error {SECRET}"),
            &[SECRET],
        )
        .await
        .unwrap();
    let log = PgLogStore::new(pool.clone());
    log.append(
        labrys_core::LogSource::Capability,
        labrys_core::LogLevel::Error,
        &Correlation::new("trace-1"),
        &format!("handshake failed {SECRET}"),
        &[SECRET],
        Utc::now(),
    )
    .await
    .unwrap();
    let loaded_audit = audit.load().await.unwrap();
    for record in loaded_audit.records() {
        assert!(!record.detail.contains(SECRET));
    }
    let logs = log
        .for_source(labrys_core::LogSource::Capability)
        .await
        .unwrap();
    assert_eq!(logs.len(), 1);
    assert!(!logs[0].message.contains(SECRET));
}

#[tokio::test]
async fn audit_ordering_survives_concurrent_writes_and_detects_tampering() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let audit = PgAuditLog::new(pool.clone());
    let mut tasks = Vec::new();
    for i in 0..8 {
        let audit = audit.clone();
        tasks.push(tokio::spawn(async move {
            audit
                .append(
                    Utc::now(),
                    &format!("worker-{i}"),
                    "action",
                    &format!("target-{i}"),
                    "detail",
                    &[],
                )
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let log = audit.load().await.unwrap();
    assert_eq!(log.len(), 8);
    log.verify().expect("chain intact after concurrent appends");
    let sequences: Vec<u64> = log.records().iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, (1..=8).collect::<Vec<u64>>());

    // The append-only trigger refuses tampering (UPDATE/DELETE).
    let tamper = sqlx::query("UPDATE audit_log SET detail = 'x' WHERE sequence = 1")
        .execute(&pool)
        .await;
    assert!(tamper.is_err(), "audit_log must reject UPDATE");
}

// ---------------------------------------------------------------------------
// Transactionally coherent persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_write_rolls_back_with_desired_state_on_conflict() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let store = PgApplicationStore::new(pool.clone());
    let events = PgEventStore::new(pool.clone());
    let app = sample_application();
    store.save(&app).await.unwrap();

    let next = app
        .desired_state
        .with_config(serde_json::json!({"ok": true}));
    store
        .update_desired_state(
            &app.id,
            1,
            &next,
            &reconcile_event(&app.id),
            "v2",
            "tester",
            &[],
        )
        .await
        .unwrap();
    let baseline = events
        .for_application(&app.id.to_string())
        .await
        .unwrap()
        .len();

    // A conflicting (stale) mutation must leave both state and event unchanged.
    let stale = next.with_config(serde_json::json!({"bad": true}));
    let err = store
        .update_desired_state(&app.id, 1, &stale, &reconcile_event(&app.id), "x", "y", &[])
        .await;
    assert!(
        matches!(err, Err(ControlPlaneError::Conflict(_))),
        "{err:?}"
    );

    let after = events
        .for_application(&app.id.to_string())
        .await
        .unwrap()
        .len();
    assert_eq!(after, baseline, "rolled-back mutation wrote no event");
    assert_eq!(store.get(&app.id).await.unwrap().desired_state.version, 2);
}

// ---------------------------------------------------------------------------
// Durable, idempotent jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_workers_claim_one_job_exactly_once() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let queue = PgJobQueue::new(pool.clone());
    let target = "res_shared";
    queue
        .enqueue(
            target,
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();

    let queue_a = queue.clone();
    let queue_b = queue.clone();
    let now = Utc::now();
    let (a, b) = tokio::join!(
        queue_a.claim_next("worker-a", now, Duration::seconds(30)),
        queue_b.claim_next("worker-b", now, Duration::seconds(30)),
    );
    let claimed = [a.unwrap(), b.unwrap()]
        .iter()
        .filter(|c| c.is_some())
        .count();
    assert_eq!(claimed, 1, "exactly one worker must claim the job");
}

#[tokio::test]
async fn duplicate_request_returns_existing_outcome() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let queue = PgJobQueue::new(pool.clone());
    let first = queue
        .enqueue(
            "res_dup",
            JobAction::Update,
            5,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    let id = match first {
        labrys_core::EnqueueOutcome::Enqueued(id) => id,
        other => panic!("expected Enqueued, got {other:?}"),
    };
    let second = queue
        .enqueue(
            "res_dup",
            JobAction::Update,
            5,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    match second {
        labrys_core::EnqueueOutcome::Duplicate(existing) => assert_eq!(existing, id),
        other => panic!("expected Duplicate, got {other:?}"),
    }
    let count: i64 = sqlx::query("SELECT count(*) FROM jobs WHERE idempotency_key = $1")
        .bind(IdempotencyKey::new("res_dup", JobAction::Update, 5).as_str())
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!(count, 1, "no second side effect is enqueued");
}

#[tokio::test]
async fn interrupted_job_recovers_without_false_readiness() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let app_store = PgApplicationStore::new(pool.clone());
    let resource_store = PgResourceStore::new(pool.clone());
    let app = sample_application();
    app_store.save(&app).await.unwrap();
    let env_id = app.environment_by_name("production").unwrap().id;
    let mut resource = Resource::request("database.postgres", 1, Utc::now()).unwrap();
    resource
        .observe(
            ResourcePhase::Provisioning,
            ObservationSource::PlatformProbe,
            None,
            Utc::now(),
        )
        .unwrap();
    let resource_id = resource.id;
    resource_store
        .save(&resource, &app.id, Some(&env_id))
        .await
        .unwrap();

    let queue = PgJobQueue::new(pool.clone());
    queue
        .enqueue(
            resource_id.to_string(),
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();

    // Worker 1 claims then "crashes" (never completes) with a 1s lease.
    let claimed = queue
        .claim_next("worker-crash", Utc::now(), Duration::seconds(1))
        .await
        .unwrap()
        .expect("claim");
    assert_eq!(claimed.job.status, JobStatus::Running);

    // After the lease expires, recovery makes it claimable again.
    let later = Utc::now() + Duration::seconds(5);
    let recovered = queue.recover_expired_leases(later).await.unwrap();
    assert_eq!(recovered, vec![claimed.job.id]);
    let reclaimed = queue
        .claim_next("worker-recover", later, Duration::seconds(30))
        .await
        .unwrap()
        .expect("re-claim after recovery");
    assert_eq!(reclaimed.job.attempts, 2, "attempt history is preserved");

    // The resource is still provisioning: recovery never implies readiness.
    let resource = resource_store.get(&resource_id).await.unwrap();
    assert_eq!(resource.phase, ResourcePhase::Provisioning);
    assert!(!resource.is_ready());
}

#[tokio::test]
async fn agent_completion_claim_never_promotes_readiness() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let app_store = PgApplicationStore::new(pool.clone());
    let resource_store = PgResourceStore::new(pool.clone());
    let app = sample_application();
    app_store.save(&app).await.unwrap();
    let env_id = app.environment_by_name("production").unwrap().id;
    let mut resource = Resource::request("database.postgres", 1, Utc::now()).unwrap();
    resource
        .observe(
            ResourcePhase::Provisioning,
            ObservationSource::PlatformProbe,
            None,
            Utc::now(),
        )
        .unwrap();
    let resource_id = resource.id;
    resource_store
        .save(&resource, &app.id, Some(&env_id))
        .await
        .unwrap();

    let queue = PgJobQueue::new(pool.clone());
    queue
        .enqueue(
            resource_id.to_string(),
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();

    // An agent "accepted" claim: the dispatcher reports InProgress (never a
    // platform readiness observation), so the resource stays provisioning.
    let worker = Worker::new(
        pool.clone(),
        std::sync::Arc::new(ScriptedDispatcher::new(vec![DispatchOutcome::InProgress])),
        &test_config("agent-claim"),
    );
    worker.run_bounded(1).await.unwrap();
    assert_eq!(
        resource_store.get(&resource_id).await.unwrap().phase,
        ResourcePhase::Provisioning
    );

    // Only a platform-owned readiness observation promotes it.
    queue
        .enqueue(
            resource_id.to_string(),
            JobAction::Verify,
            2,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    let ready_worker = Worker::new(
        pool.clone(),
        std::sync::Arc::new(ScriptedDispatcher::new(vec![DispatchOutcome::Ready(
            resource_id,
        )])),
        &test_config("platform-ready"),
    );
    ready_worker.run_bounded(1).await.unwrap();
    assert!(resource_store.get(&resource_id).await.unwrap().is_ready());
}

#[tokio::test]
async fn retry_limit_exhaustion_dead_letters_with_recovery_guidance() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let queue = PgJobQueue::new(pool.clone());
    let policy = RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::seconds(1),
        factor: 2,
        max_delay: None,
    };
    queue
        .enqueue("res_fail", JobAction::Provision, 1, policy, Utc::now())
        .await
        .unwrap();

    // Attempt 1 fails -> requeued.
    let c1 = queue
        .claim_next("w", Utc::now(), Duration::seconds(30))
        .await
        .unwrap()
        .unwrap();
    let status = queue
        .fail(&c1.job.id, "w", &format!("boom {SECRET}"), Utc::now())
        .await
        .unwrap();
    assert_eq!(status, JobStatus::Queued);

    // Attempt 2 fails -> dead-lettered (attempts reached max).
    let later = Utc::now() + Duration::seconds(10);
    let c2 = queue
        .claim_next("w", later, Duration::seconds(30))
        .await
        .unwrap()
        .unwrap();
    let status = queue
        .fail(&c2.job.id, "w", "boom again", later)
        .await
        .unwrap();
    assert_eq!(status, JobStatus::Dead);

    let dead = queue.dead_letters().await.unwrap();
    assert_eq!(dead, vec![c2.job.id]);
    let job = queue.get(&c2.job.id).await.unwrap();
    assert_eq!(job.attempts, 2);
    assert_eq!(job.last_error.as_deref(), Some("boom again"));

    // Explicit requeue resets the budget.
    queue.requeue(&c2.job.id, later).await.unwrap();
    let job = queue.get(&c2.job.id).await.unwrap();
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.attempts, 0);
}

#[tokio::test]
async fn pause_and_resume_gate_claiming() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let queue = PgJobQueue::new(pool.clone());
    let outcome = queue
        .enqueue(
            "res_pause",
            JobAction::Update,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    let id = match outcome {
        labrys_core::EnqueueOutcome::Enqueued(id) => id,
        _ => panic!("enqueue"),
    };
    queue.pause(&id, Utc::now()).await.unwrap();
    assert!(queue
        .claim_next("w", Utc::now(), Duration::seconds(30))
        .await
        .unwrap()
        .is_none());
    queue.resume(&id, Utc::now()).await.unwrap();
    assert!(queue
        .claim_next("w", Utc::now(), Duration::seconds(30))
        .await
        .unwrap()
        .is_some());
}

// ---------------------------------------------------------------------------
// Shutdown and observability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graceful_shutdown_stops_the_run_loop() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let worker = std::sync::Arc::new(Worker::new(
        pool.clone(),
        std::sync::Arc::new(NoopDispatcher),
        &test_config("shutdown"),
    ));
    let runner = std::sync::Arc::clone(&worker);
    let handle = tokio::spawn(async move { runner.run().await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    worker.stop();
    tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("worker exits after shutdown")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn worker_transitions_are_attributed_and_redacted() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let queue = PgJobQueue::new(pool.clone());
    queue
        .enqueue(
            "res_attr",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    let worker = Worker::with_secrets(
        pool.clone(),
        std::sync::Arc::new(ScriptedDispatcher::new(vec![DispatchOutcome::Failed(
            format!("provider refused {SECRET}"),
        )])),
        &test_config("attr-worker"),
        vec![SECRET.to_string()],
    );
    let stats = worker.run_bounded(1).await.unwrap();
    assert_eq!(stats.retried, 1);

    let events = PgEventStore::new(pool.clone());
    let trace = events.for_trace("worker:attr-worker").await.unwrap();
    assert!(trace
        .iter()
        .any(|e| matches!(e.result, EventResult::Failed { .. })));
    for event in &trace {
        assert!(!format!("{event:?}").contains(SECRET));
    }
}

// ---------------------------------------------------------------------------
// Evidence and usage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn evidence_retention_keeps_unresolved_failures() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let policy = RetentionPolicy {
        max_age: Duration::days(1),
        keep_latest: 2,
    };
    let store = PgEvidenceStore::new(pool.clone(), policy);
    let old = Utc::now() - Duration::days(10);
    for i in 0..4 {
        let e = VerificationEvidence::new(
            format!("pass-{i}"),
            VerifierStage::Deterministic,
            "schema",
            true,
            "ok",
            &[],
            old,
        );
        store.append(&e, &[]).await.unwrap();
    }
    let failure = VerificationEvidence::new(
        "fail-1",
        VerifierStage::Health,
        "probe",
        false,
        "unreachable",
        &[],
        Utc::now(),
    );
    store.append(&failure, &[]).await.unwrap();

    let removed = store.retain(Utc::now()).await.unwrap();
    assert_eq!(removed.len(), 4, "expired passing evidence pruned");
    let remaining = store.all().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].id, "fail-1",
        "blocking failure always retained"
    );
}

#[tokio::test]
async fn usage_totals_aggregate_per_application() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let app = sample_application();
    PgApplicationStore::new(pool.clone())
        .save(&app)
        .await
        .unwrap();
    let env_id = app.environment_by_name("production").unwrap().id;
    let ledger = PgUsageLedger::new(pool.clone());
    for seconds in [10u64, 32] {
        ledger
            .record(&UsageRecord {
                application_id: app.id,
                environment_id: env_id,
                period_start: Utc::now(),
                period_end: Utc::now(),
                cpu_millicore_seconds: seconds,
                memory_mb_seconds: seconds * 2,
                build_seconds: 1,
                requests: 3,
                egress_bytes: 100,
            })
            .await
            .unwrap();
    }
    let totals = ledger.totals(&app.id.to_string()).await.unwrap();
    assert_eq!(totals.cpu_millicore_seconds, 42);
    assert_eq!(totals.memory_mb_seconds, 84);
    assert_eq!(totals.requests, 6);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executable_smoke_runs_migrations_and_one_recovery_cycle() {
    // Mirrors the control-plane binary's startup path: connect, migrate,
    // converge one job to completion, then recover a lease an interrupted
    // worker left behind. Infrastructure evidence, not a production claim.
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let app_store = PgApplicationStore::new(pool.clone());
    let app = sample_application();
    app_store.save(&app).await.unwrap();

    let queue = PgJobQueue::new(pool.clone());
    queue
        .enqueue(
            "res-smoke",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();

    // One full claim -> dispatch -> complete cycle.
    let worker = Worker::new(
        pool.clone(),
        std::sync::Arc::new(NoopDispatcher),
        &test_config("smoke"),
    );
    let stats = worker.run_bounded(1).await.unwrap();
    assert_eq!(stats.in_progress, 1);
    assert!(
        queue.is_quiet().await.unwrap(),
        "job drained to a terminal state"
    );

    // A lease an interrupted worker left behind is recovered and re-runnable.
    let claimed = queue
        .claim_next("smoke-crash", Utc::now(), Duration::seconds(1))
        .await
        .unwrap();
    assert!(
        claimed.is_none(),
        "no job is ready after the first completed"
    );
    queue
        .enqueue(
            "res-smoke-2",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    queue
        .claim_next("smoke-crash", Utc::now(), Duration::seconds(1))
        .await
        .unwrap()
        .expect("claim second job");
    let recovered = queue
        .recover_expired_leases(Utc::now() + Duration::seconds(3))
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1, "expired lease recovered");
}

fn reconcile_event(application_id: &ApplicationId) -> EventDraft {
    EventDraft::new(
        Utc::now(),
        EventActor::platform("test"),
        Correlation::new("trace-desired").with_application(*application_id),
        EventAction::Reconciled,
        EventResult::Accepted,
    )
}
