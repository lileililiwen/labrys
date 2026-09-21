//! Dispatcher-selection integration tests for the executable daemon wiring.
//!
//! Unit-level selection (disabled never probes, docker mode fails fast, auto
//! starts blocked without a runtime) runs without infrastructure. Store-backed
//! tests need `LABRYS_DATABASE_URL`/`DATABASE_URL` and skip otherwise, like
//! the rest of the suite; they are infrastructure evidence, not
//! production-deployment evidence.

use std::sync::{Arc, OnceLock};

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    connect, run_migrations, select_dispatcher, Config, DispatchOutcome, ExecutableDispatcher,
    InjectedFailure, JobDispatcher, LocalTestAdapter, PgEventStore, PgJobQueue, PgLogStore,
    ProviderJobDispatcher, ProviderRuntime, RuntimeMode, UnavailableDispatcher, Worker,
    BLOCKER_RECOVERY,
};
use labrys_core::{JobAction, LogSource, RetryPolicy};

const SECRET: &str = "hunter2-dispatch-secret-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

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

fn lazy_pool() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://127.0.0.1:1/unused")
        .expect("lazy pool")
}

fn selection_config(mode: RuntimeMode) -> Config {
    Config {
        database_url: "postgres://127.0.0.1:1/unused".to_string(),
        worker_id: "dispatch-test".to_string(),
        lease_seconds: 60,
        poll_interval_ms: 10,
        drain_timeout_ms: 200,
        max_concurrency: 1,
        run_migrations: false,
        runtime_mode: mode,
        docker_bin: std::path::PathBuf::from("/nonexistent-labrys-docker-binary"),
        workspace_root: std::env::temp_dir().join("labrys-dispatch-test"),
        preview_ttl_secs: 3_600,
        provider_postgres_url: None,
        provider_storage_root: std::env::temp_dir().join("labrys-dispatch-test-storage"),
        provider_registry_endpoint: None,
        provider_registry_username: None,
        provider_registry_password: None,
        provider_tls_dir: std::env::temp_dir().join("labrys-dispatch-test-tls"),
    }
}

fn execution_config(db_url: &str) -> Config {
    Config {
        database_url: db_url.to_string(),
        worker_id: "dispatch-worker".to_string(),
        lease_seconds: 60,
        poll_interval_ms: 10,
        drain_timeout_ms: 500,
        max_concurrency: 1,
        run_migrations: false,
        runtime_mode: RuntimeMode::Auto,
        docker_bin: std::path::PathBuf::from("docker"),
        workspace_root: std::env::temp_dir().join("labrys-dispatch-test"),
        preview_ttl_secs: 3_600,
        provider_postgres_url: None,
        provider_storage_root: std::env::temp_dir().join("labrys-dispatch-test-storage"),
        provider_registry_endpoint: None,
        provider_registry_username: None,
        provider_registry_password: None,
        provider_tls_dir: std::env::temp_dir().join("labrys-dispatch-test-tls"),
    }
}

// ---------------------------------------------------------------------------
// Selection without infrastructure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disabled_mode_never_probes_and_starts_blocked() {
    let pool = lazy_pool();
    let selected = select_dispatcher(&pool, &selection_config(RuntimeMode::Disabled))
        .await
        .unwrap();
    assert!(selected.blocked);
    assert!(!selected.availability.is_available());
    let err = selected
        .availability
        .require()
        .expect_err("disabled must be unavailable");
    assert!(err.to_string().contains("disabled"), "{err}");
}

#[tokio::test]
async fn docker_mode_fails_startup_fast_without_a_runtime() {
    let pool = lazy_pool();
    let err = select_dispatcher(&pool, &selection_config(RuntimeMode::Docker))
        .await
        .expect_err("explicit docker mode without a runtime must fail startup");
    assert!(err.to_string().contains("unreachable"), "{err}");
}

#[tokio::test]
async fn auto_mode_starts_blocked_without_a_runtime() {
    let pool = lazy_pool();
    let selected = select_dispatcher(&pool, &selection_config(RuntimeMode::Auto))
        .await
        .unwrap();
    assert!(selected.blocked);
    assert!(!selected.availability.is_available());
}

// ---------------------------------------------------------------------------
// Blocker reporting with persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unavailable_dispatcher_reports_blocker_with_recovery_and_redaction() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let dispatcher =
        UnavailableDispatcher::new(pool.clone(), "docker runtime is not available in this test")
            .with_secrets(vec![SECRET.to_string()]);
    let queue = PgJobQueue::new(pool.clone());
    let now = Utc::now();
    let target = format!("leaky-target:{SECRET}");
    let job = match queue
        .enqueue(
            &target,
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            now,
        )
        .await
        .unwrap()
    {
        labrys_core::EnqueueOutcome::Enqueued(id) => queue.get(&id).await.unwrap(),
        labrys_core::EnqueueOutcome::Duplicate(id) => queue.get(&id).await.unwrap(),
    };
    let outcome = dispatcher.dispatch(&job).await.unwrap();
    let reason = match outcome {
        DispatchOutcome::Failed(reason) => reason,
        other => panic!("blocked dispatch must fail, got {other:?}"),
    };
    assert!(reason.contains("blocked"), "{reason}");
    assert!(reason.contains(BLOCKER_RECOVERY), "{reason}");
    assert!(!reason.contains(SECRET), "blocker reason leaked a secret");

    let events = PgEventStore::new(pool.clone());
    let stored = events.for_trace(&format!("job:{}", job.id)).await.unwrap();
    assert_eq!(stored.len(), 1, "one attributable blocker event");
    let detail = format!("{:?}", stored[0]);
    assert!(!detail.contains(SECRET), "blocker event leaked a secret");

    let logs = PgLogStore::new(pool.clone());
    let runtime_logs = logs.for_source(LogSource::Runtime).await.unwrap();
    assert!(
        runtime_logs
            .iter()
            .any(|entry| entry.correlation.trace_id == format!("job:{}", job.id)),
        "blocker is in the separated runtime log under the job trace"
    );
    assert!(
        runtime_logs
            .iter()
            .all(|entry| !entry.message.contains(SECRET)),
        "runtime log leaked a secret"
    );
}

// ---------------------------------------------------------------------------
// Real dispatch with identity plumbing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn executable_dispatcher_accepts_provisioning_with_job_identity() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let db_url = database_url().expect("db url");
    let config = execution_config(&db_url);
    let dispatcher = ExecutableDispatcher::build(pool.clone(), &config)
        .unwrap()
        .with_availability(labrys_control_plane::RuntimeAvailability::Available {
            docker_version: "test docker".to_string(),
        });
    // The daemon carries its executors and posture together.
    assert!(dispatcher.availability().is_available());
    assert!(dispatcher.preview_ttl_secs() == 3_600);
    let _ = dispatcher.executor();
    let _ = dispatcher.preview_manager();

    let queue = PgJobQueue::new(pool.clone());
    let now = Utc::now();
    let target = "generic.local-test-resource";
    let job = match queue
        .enqueue(target, JobAction::Provision, 1, RetryPolicy::default(), now)
        .await
        .unwrap()
    {
        labrys_core::EnqueueOutcome::Enqueued(id) => queue.get(&id).await.unwrap(),
        labrys_core::EnqueueOutcome::Duplicate(id) => queue.get(&id).await.unwrap(),
    };
    let outcome = dispatcher.dispatch(&job).await.unwrap();
    assert_eq!(outcome, DispatchOutcome::InProgress);

    let events = PgEventStore::new(pool.clone());
    let stored = events.for_trace(&format!("job:{}", job.id)).await.unwrap();
    assert!(
        stored.len() >= 2,
        "provider observation plus the identity-attributed dispatch stage"
    );
    let trace = format!("job:{}", job.id);
    assert!(
        stored
            .iter()
            .all(|event| event.correlation.trace_id == trace),
        "every dispatch event carries the job trace"
    );
}

#[tokio::test]
async fn executable_dispatcher_redacts_provider_failures() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let db_url = database_url().expect("db url");
    let config = execution_config(&db_url);
    let failing = Arc::new(
        LocalTestAdapter::new("leaky.local-test", labrys_core::ProviderKind::Generic)
            .with_failures(vec![InjectedFailure::CredentialLeak(SECRET.to_string())]),
    );
    let provider = ProviderJobDispatcher::new(ProviderRuntime::with_secrets(
        pool.clone(),
        vec![SECRET.to_string()],
    ))
    .with_adapter(failing);
    let dispatcher = ExecutableDispatcher::build(pool.clone(), &config)
        .unwrap()
        .with_provider(provider)
        .with_secrets(vec![SECRET.to_string()])
        .with_availability(labrys_control_plane::RuntimeAvailability::Available {
            docker_version: "test docker".to_string(),
        });

    let queue = PgJobQueue::new(pool.clone());
    let now = Utc::now();
    let job = match queue
        .enqueue(
            "leaky.local-test-resource",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            now,
        )
        .await
        .unwrap()
    {
        labrys_core::EnqueueOutcome::Enqueued(id) => queue.get(&id).await.unwrap(),
        labrys_core::EnqueueOutcome::Duplicate(id) => queue.get(&id).await.unwrap(),
    };
    let outcome = dispatcher.dispatch(&job).await.unwrap();
    let reason = match outcome {
        DispatchOutcome::Failed(reason) => reason,
        other => panic!("leaking provider must surface failure, got {other:?}"),
    };
    assert!(!reason.contains(SECRET), "dispatch failure leaked a secret");

    let events = PgEventStore::new(pool.clone());
    let stored = events.for_trace(&format!("job:{}", job.id)).await.unwrap();
    assert!(!stored.is_empty());
    for event in &stored {
        assert!(
            !format!("{:?}", event).contains(SECRET),
            "persisted dispatch evidence leaked a secret"
        );
    }
}

// ---------------------------------------------------------------------------
// Worker loop over the wired dispatchers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn worker_accepts_jobs_through_the_executable_dispatcher() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let db_url = database_url().expect("db url");
    let config = execution_config(&db_url);
    let dispatcher = Arc::new(
        ExecutableDispatcher::build(pool.clone(), &config)
            .unwrap()
            .with_availability(labrys_control_plane::RuntimeAvailability::Available {
                docker_version: "test docker".to_string(),
            }),
    );
    let queue = PgJobQueue::new(pool.clone());
    queue
        .enqueue(
            "generic.local-test-worker",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap();
    let worker = Worker::new(pool.clone(), dispatcher, &config);
    let stats = worker.run_bounded(5).await.unwrap();
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.in_progress, 1);
    assert_eq!(stats.retried, 0);
}

#[tokio::test]
async fn worker_retries_blocked_jobs_with_the_blocker_reason() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let db_url = database_url().expect("db url");
    let config = execution_config(&db_url);
    let dispatcher: Arc<dyn JobDispatcher> = Arc::new(UnavailableDispatcher::new(
        pool.clone(),
        "no container runtime in this test",
    ));
    let queue = PgJobQueue::new(pool.clone());
    let id = match queue
        .enqueue(
            "blocked-worker-target",
            JobAction::Provision,
            1,
            RetryPolicy::default(),
            Utc::now(),
        )
        .await
        .unwrap()
    {
        labrys_core::EnqueueOutcome::Enqueued(id) => id,
        labrys_core::EnqueueOutcome::Duplicate(id) => id,
    };
    let worker = Worker::new(pool.clone(), dispatcher, &config);
    let stats = worker.run_bounded(5).await.unwrap();
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.retried, 1);
    let job = queue.get(&id).await.unwrap();
    let last_error = job.last_error.expect("blocked job records its error");
    assert!(last_error.contains("blocked"), "{last_error}");
    assert!(last_error.contains(BLOCKER_RECOVERY), "{last_error}");
}

#[tokio::test]
async fn executable_smoke_runs_migrations_and_reports_selection() {
    let Some((_g, _pool)) = setup().await else {
        return;
    };
    let db_url = database_url().expect("db url");
    let config = execution_config(&db_url);
    let pool = connect(&config).await.unwrap();
    run_migrations(&pool).await.unwrap();
    // Disabled mode proves the blocked daemon path end to end without Docker.
    let mut blocked_config = config.clone();
    blocked_config.runtime_mode = RuntimeMode::Disabled;
    let selected = select_dispatcher(&pool, &blocked_config).await.unwrap();
    assert!(selected.blocked);
}
