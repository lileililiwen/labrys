use chrono::{Duration, TimeZone, Utc};

use labrys_core::{
    aggregate_health, redact_reason, ApplicationController, CapabilityController, Controller,
    DeploymentController, DriftKind, EnqueueOutcome, HealthCheck, HealthReport, HealthState,
    IdempotencyKey, JobAction, JobQueue, JobStatus, ObservationSource, PreviewController,
    Reconciliation, Resource, ResourceController, ResourcePhase, RetryPolicy,
};
use labrys_core::{ApplicationId, ObservedState, ObservedStatus, PreviewId, ResourceId};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn default_policy() -> RetryPolicy {
    RetryPolicy::default()
}

fn postgres_resource() -> Resource {
    Resource::request("database.postgres", 1, now()).unwrap()
}

// --- Requirement: desired and actual state are separate ---

#[test]
fn application_controller_converges_desired_state_after_the_request_ends() {
    let desired = labrys_core::DesiredState::initial();
    let observed = ObservedState::unknown(desired.version);
    let mut controller =
        ApplicationController::new(ApplicationId::new(), desired.clone(), observed);
    let mut queue = JobQueue::new();

    // Reconciliation runs on its own schedule, not inside the request.
    let first = controller.reconcile(&mut queue, now()).unwrap();
    assert!(first.is_mismatch());
    assert!(first.drift.iter().any(|d| d.kind == DriftKind::Unhealthy)); // unknown != healthy
    assert_eq!(queue.len(), 1);

    // A second pass must not duplicate the in-flight job.
    let second = controller
        .reconcile(&mut queue, now() + Duration::minutes(1))
        .unwrap();
    assert!(second.is_mismatch());
    assert_eq!(queue.len(), 1);
    assert_eq!(first.actions, second.actions);

    // The worker converges; a platform observation closes the loop.
    let job_id = queue.claim_next(now()).unwrap();
    queue.complete(job_id, now()).unwrap();
    let healthy = ObservedState {
        desired_version: desired.version,
        status: ObservedStatus::Healthy,
        last_observed_at: now(),
        detail: None,
    };
    controller.observe(healthy);
    let third = controller.reconcile(&mut queue, now()).unwrap();
    assert!(third.in_sync);
    assert!(third.drift.is_empty());
    assert!(third.actions.is_empty());
}

#[test]
fn disappearing_database_reports_mismatch_until_recovery_is_observed() {
    let mut resource = postgres_resource();
    resource
        .observe(
            ResourcePhase::Provisioning,
            ObservationSource::ProviderCallback,
            None,
            now(),
        )
        .unwrap();
    resource
        .observe(
            ResourcePhase::Ready,
            ObservationSource::PlatformProbe,
            None,
            now(),
        )
        .unwrap();
    let mut controller = ResourceController::new(resource);
    controller
        .observe(
            true,
            Some(ResourcePhase::Ready),
            ObservationSource::PlatformProbe,
            None,
            now(),
        )
        .unwrap();
    let mut queue = JobQueue::new();

    // A successful request converged the resource ...
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);

    // WHEN the database disappears afterwards ...
    controller
        .observe(
            false,
            Some(ResourcePhase::Failed),
            ObservationSource::PlatformProbe,
            Some("connection refused".to_string()),
            now(),
        )
        .unwrap();
    let pass = controller.reconcile(&mut queue, now()).unwrap();

    // THEN an idempotent provisioning action is enqueued ...
    assert!(pass.is_mismatch());
    assert!(pass.drift.iter().any(|d| d.kind == DriftKind::Missing));
    assert_eq!(queue.len(), 1);
    let again = controller.reconcile(&mut queue, now()).unwrap();
    assert_eq!(again.actions, pass.actions);
    assert_eq!(queue.len(), 1);

    // AND the mismatch is reported until recovery is observed.
    let job = queue.claim_next(now()).unwrap();
    queue.complete(job, now()).unwrap();
    controller
        .observe(
            true,
            Some(ResourcePhase::Provisioning),
            ObservationSource::ProviderCallback,
            None,
            now(),
        )
        .unwrap();
    assert!(controller
        .reconcile(&mut queue, now())
        .unwrap()
        .is_mismatch());
    controller
        .observe(
            true,
            Some(ResourcePhase::Ready),
            ObservationSource::PlatformProbe,
            None,
            now(),
        )
        .unwrap();
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);
    assert_eq!(controller.health(), HealthState::Healthy);
}

// --- Requirement: explicit resource lifecycle ---

#[test]
fn lifecycle_walks_the_explicit_graph_with_evidence() {
    let mut resource = postgres_resource();
    assert_eq!(resource.phase, ResourcePhase::Requested);

    let path = [
        ResourcePhase::Provisioning,
        ResourcePhase::Ready,
        ResourcePhase::Degraded,
        ResourcePhase::Ready,
        ResourcePhase::Deleting,
        ResourcePhase::Deleted,
    ];
    for (i, phase) in path.iter().enumerate() {
        resource
            .observe(
                *phase,
                ObservationSource::PlatformProbe,
                None,
                now() + Duration::seconds(i as i64),
            )
            .unwrap();
    }
    assert!(resource.phase.is_terminal());
    // Initial request + six transitions.
    assert_eq!(resource.transitions.len(), 7);
    assert_eq!(resource.transitions[1].to, ResourcePhase::Provisioning);
    assert_eq!(
        resource.transitions.last().unwrap().to,
        ResourcePhase::Deleted
    );

    // Deleted is terminal; illegal jumps are rejected.
    assert!(resource
        .observe(
            ResourcePhase::Requested,
            ObservationSource::PlatformProbe,
            None,
            now()
        )
        .is_err());
    let mut skipper = postgres_resource();
    assert!(skipper
        .observe(
            ResourcePhase::Deleted,
            ObservationSource::PlatformProbe,
            None,
            now()
        )
        .is_err());
    assert!(skipper
        .observe(
            ResourcePhase::Requested,
            ObservationSource::PlatformProbe,
            None,
            now()
        )
        .is_err());
}

#[test]
fn provisioning_failure_records_redacted_reason_and_retry_policy() {
    let mut resource = postgres_resource();
    resource
        .observe(
            ResourcePhase::Provisioning,
            ObservationSource::ProviderCallback,
            None,
            now(),
        )
        .unwrap();

    // WHEN a provider returns a terminal provisioning error carrying a secret.
    resource
        .fail(
            "auth failed for password hunter2 against db.internal",
            &["hunter2"],
            default_policy(),
            now(),
        )
        .unwrap();

    // THEN the resource is failed with a redacted reason and a retry policy.
    assert_eq!(resource.phase, ResourcePhase::Failed);
    let failure = resource.failure.clone().unwrap();
    assert_eq!(failure.attempts, 1);
    assert_eq!(failure.policy, default_policy());
    assert_eq!(failure.next_retry_at, now() + failure.policy.delay_for(1));
    assert!(!failure.reason.contains("hunter2"));
    assert!(!resource.to_event_detail().contains("hunter2"));
    assert!(resource.to_event_detail().contains("[redacted]"));
    assert_eq!(resource.generation, 1);

    // AND dependent bindings do not claim healthy state.
    let capability = CapabilityController::new("database.postgres", resource.id);
    let mut dependent = capability;
    dependent.observe_readiness(resource.phase);
    assert_ne!(dependent.health(), HealthState::Healthy);
    assert_eq!(dependent.health(), HealthState::Failed);
}

// --- Requirement: agent completion cannot replace reconciliation ---

#[test]
fn agent_tool_result_never_proves_readiness() {
    let mut controller = CapabilityController::new("database.postgres", ResourceId::new());
    let mut queue = JobQueue::new();

    // WHEN a provisioning tool returns an accepted request ...
    controller.accept_agent_completion();

    // THEN the capability remains provisioning ...
    assert_eq!(controller.phase, ResourcePhase::Provisioning);
    assert_eq!(controller.health(), HealthState::Provisioning);
    assert_eq!(controller.agent_claims_ignored, 1);

    // ... and reconciliation keeps converging it.
    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch());
    assert_eq!(queue.len(), 1);

    // Only a platform observation of readiness closes it.
    controller.observe_readiness(ResourcePhase::Ready);
    let done = controller.reconcile(&mut queue, now()).unwrap();
    assert!(done.in_sync);
    assert_eq!(controller.health(), HealthState::Healthy);
}

#[test]
fn health_report_excludes_agent_claims() {
    let checks = vec![
        HealthCheck::agent_claim("agent-says-healthy", HealthState::Healthy),
        HealthCheck::probe("db-probe", HealthState::Unknown),
    ];
    let report = HealthReport::from_checks(checks, now());

    // An agent assertion cannot outvote a missing platform check.
    assert_eq!(report.state, HealthState::Unknown);
    assert_eq!(
        report.ignored_agent_claims,
        vec!["agent-says-healthy".to_string()]
    );
    assert_eq!(report.checks.len(), 1);

    let healthy = HealthReport::from_checks(
        vec![
            HealthCheck::probe("db-probe", HealthState::Healthy),
            HealthCheck::probe("app-probe", HealthState::Healthy),
        ],
        now(),
    );
    assert!(healthy.is_healthy());
}

#[test]
fn health_precedence_is_deterministic() {
    use HealthState::*;
    let cases: Vec<(Vec<HealthState>, HealthState)> = vec![
        (vec![], Unknown),
        (vec![Healthy, Healthy], Healthy),
        (vec![Healthy, Paused], Paused),
        (vec![Healthy, Provisioning], Provisioning),
        (vec![Provisioning, Unknown], Unknown),
        (vec![Healthy, Degraded], Degraded),
        (vec![Degraded, Failed], Failed),
        (vec![Failed, Healthy, Unknown], Failed),
    ];
    for (inputs, expected) in cases {
        assert_eq!(aggregate_health(inputs.into_iter()), expected);
    }
}

// --- Jobs: retries, duplicates, pause, dead-letter, recovery ---

#[test]
fn idempotency_keys_are_deterministic_and_version_scoped() {
    let a = IdempotencyKey::new("res_x", JobAction::Provision, 1);
    let b = IdempotencyKey::new("res_x", JobAction::Provision, 1);
    let c = IdempotencyKey::new("res_x", JobAction::Provision, 2);
    let d = IdempotencyKey::new("res_x", JobAction::Delete, 1);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
    assert_eq!(a.to_string(), "res_x:provision:v1");
}

#[test]
fn duplicate_jobs_are_suppressed_until_terminal() {
    let mut queue = JobQueue::new();
    let first = queue.enqueue("res_x", JobAction::Provision, 1, default_policy(), now());
    let EnqueueOutcome::Enqueued(id) = first else {
        panic!("first enqueue must create a job")
    };
    assert_eq!(
        queue.enqueue("res_x", JobAction::Provision, 1, default_policy(), now()),
        EnqueueOutcome::Duplicate(id)
    );
    assert_eq!(queue.len(), 1);

    // A new generation (different version) is a different key.
    assert!(matches!(
        queue.enqueue("res_x", JobAction::Provision, 2, default_policy(), now()),
        EnqueueOutcome::Enqueued(_)
    ));

    // After both jobs reach a terminal state the key is free again.
    let j1 = queue.claim_next(now()).unwrap();
    let j2 = queue.claim_next(now()).unwrap();
    queue.complete(j1, now()).unwrap();
    queue.complete(j2, now()).unwrap();
    assert!(matches!(
        queue.enqueue("res_x", JobAction::Provision, 1, default_policy(), now()),
        EnqueueOutcome::Enqueued(_)
    ));
}

#[test]
fn retries_use_exponential_backoff_and_are_not_claimed_early() {
    let mut queue = JobQueue::new();
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::seconds(5),
        factor: 2,
        max_delay: Some(Duration::seconds(30)),
    };
    queue.enqueue("res_x", JobAction::Provision, 1, policy, now());

    let job = queue.claim_next(now()).unwrap();
    assert_eq!(queue.get(job).unwrap().attempts, 1);
    assert_eq!(queue.claim_next(now()), None);

    // First failure: retry after base delay.
    let status = queue.fail(job, "connection reset", now()).unwrap();
    assert_eq!(status, JobStatus::Queued);
    assert_eq!(
        queue.get(job).unwrap().next_attempt_at,
        now() + Duration::seconds(5)
    );
    assert_eq!(queue.claim_next(now() + Duration::seconds(4)), None);

    // Second failure: doubled delay, capped by max_delay.
    let job2 = queue.claim_next(now() + Duration::seconds(5)).unwrap();
    assert_eq!(job2, job);
    queue
        .fail(job2, "timeout", now() + Duration::seconds(5))
        .unwrap();
    assert_eq!(
        queue.get(job).unwrap().next_attempt_at,
        now() + Duration::seconds(15)
    );
    let job3 = queue.claim_next(now() + Duration::seconds(15)).unwrap();
    queue
        .fail(job3, "still down", now() + Duration::seconds(15))
        .unwrap();

    // Third failure exhausts the budget: dead-lettered, not re-queued.
    assert_eq!(queue.get(job).unwrap().status, JobStatus::Dead);
    assert_eq!(queue.dead_letters(), vec![job]);
    assert_eq!(queue.claim_next(now() + Duration::hours(1)), None);
    // A dead letter is terminal, so no work remains in flight.
    assert!(queue.is_quiet());
}

#[test]
fn dead_letter_recovery_completes_after_requeue() {
    let mut queue = JobQueue::new();
    let policy = RetryPolicy {
        max_attempts: 1,
        base_delay: Duration::seconds(1),
        factor: 2,
        max_delay: None,
    };
    queue.enqueue("res_x", JobAction::Provision, 1, policy, now());
    let job = queue.claim_next(now()).unwrap();
    queue.fail(job, "provider quota", now()).unwrap();
    assert_eq!(queue.get(job).unwrap().status, JobStatus::Dead);

    // Only dead jobs can be requeued; requeue resets the attempt budget.
    assert!(queue.pause(job, now()).is_err());
    queue.requeue(job, now()).unwrap();
    assert_eq!(queue.get(job).unwrap().attempts, 0);
    let again = queue.claim_next(now()).unwrap();
    queue.complete(again, now()).unwrap();
    assert_eq!(queue.get(job).unwrap().status, JobStatus::Succeeded);
    assert!(queue.is_quiet());
}

#[test]
fn pause_holds_a_job_until_resumed() {
    let mut queue = JobQueue::new();
    queue.enqueue("res_x", JobAction::Provision, 1, default_policy(), now());
    let job = queue.jobs().next().unwrap().id;

    queue.pause(job, now()).unwrap();
    assert_eq!(queue.claim_next(now()), None);
    assert_eq!(queue.get(job).unwrap().status, JobStatus::Paused);
    // Paused jobs still hold their idempotency key.
    assert_eq!(
        queue.enqueue("res_x", JobAction::Provision, 1, default_policy(), now()),
        EnqueueOutcome::Duplicate(job)
    );

    queue.resume(job, now()).unwrap();
    assert_eq!(queue.claim_next(now()), Some(job));
    assert!(queue.pause(job, now()).is_err()); // running, not queued
}

#[test]
fn partial_failure_keeps_the_system_not_healthy() {
    let mut queue = JobQueue::new();
    // Two resources need provisioning; one converges, one dead-letters.
    queue.enqueue("res_a", JobAction::Provision, 1, default_policy(), now());
    queue.enqueue(
        "res_b",
        JobAction::Provision,
        1,
        RetryPolicy {
            max_attempts: 1,
            ..default_policy()
        },
        now() + Duration::seconds(1),
    );

    let a = queue.claim_next(now()).unwrap();
    assert_eq!(a, queue.jobs().find(|j| j.target == "res_a").unwrap().id);
    queue.complete(a, now()).unwrap();
    let b = queue.claim_next(now() + Duration::seconds(1)).unwrap();
    assert_eq!(b, queue.jobs().find(|j| j.target == "res_b").unwrap().id);
    queue
        .fail(b, "disk full", now() + Duration::seconds(1))
        .unwrap();

    let report = HealthReport::from_checks(
        vec![
            HealthCheck::probe("res_a", HealthState::Healthy),
            HealthCheck::probe("res_b", HealthState::Failed),
        ],
        now(),
    );

    // One failed platform check dominates the healthy one.
    assert_eq!(report.state, HealthState::Failed);
    assert_eq!(queue.dead_letters().len(), 1);
    // The queue is drained, yet the system is still unhealthy: a quiet queue
    // is not proof of health — the dead letter and the failed check are.
    assert!(queue.is_quiet());
}

// --- Deletion ---

#[test]
fn deletion_converges_to_deleted() {
    let mut resource = postgres_resource();
    resource
        .observe(
            ResourcePhase::Provisioning,
            ObservationSource::ProviderCallback,
            None,
            now(),
        )
        .unwrap();
    resource
        .observe(
            ResourcePhase::Ready,
            ObservationSource::PlatformProbe,
            None,
            now(),
        )
        .unwrap();
    let mut controller = ResourceController::new(resource);
    controller
        .observe(
            true,
            Some(ResourcePhase::Ready),
            ObservationSource::PlatformProbe,
            None,
            now(),
        )
        .unwrap();
    let mut queue = JobQueue::new();

    // Desired state no longer wants the resource.
    controller.desired_present = false;
    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch());
    assert_eq!(queue.len(), 1);
    // In-flight deletion is not duplicated.
    assert_eq!(
        controller.reconcile(&mut queue, now()).unwrap().actions,
        pass.actions
    );
    assert_eq!(queue.len(), 1);

    // The worker tears it down; the provider confirms.
    let job = queue.claim_next(now()).unwrap();
    controller.resource.begin_delete(now()).unwrap();
    assert!(controller
        .reconcile(&mut queue, now())
        .unwrap()
        .is_mismatch());
    controller.resource.mark_deleted(now()).unwrap();
    queue.complete(job, now()).unwrap();
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);
    assert_eq!(controller.health(), HealthState::Unknown);
}

// --- Preview and deployment controllers ---

#[test]
fn preview_controller_converges_liveness_and_health() {
    let mut controller = PreviewController::new(PreviewId::new());
    let mut queue = JobQueue::new();

    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch());
    assert_eq!(queue.len(), 1);

    controller.observe(true, false);
    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch());
    assert_eq!(controller.health(), HealthState::Degraded);
    assert_eq!(queue.len(), 2); // provision (in flight) + verify

    controller.observe(true, true);
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);
    assert_eq!(controller.health(), HealthState::Healthy);

    controller.desired_live = false;
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);
    assert_eq!(controller.health(), HealthState::Paused);
}

#[test]
fn deployment_controller_rollouts_to_the_desired_artifact() {
    let mut controller = DeploymentController::new("web", "sha256:aaa");
    let mut queue = JobQueue::new();

    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch());
    assert_eq!(queue.len(), 1);

    controller.observe("sha256:bbb", true);
    let pass = controller.reconcile(&mut queue, now()).unwrap();
    assert!(pass.is_mismatch()); // wrong artifact still running
    assert_eq!(controller.health(), HealthState::Degraded);

    controller.observe("sha256:aaa", true);
    assert!(controller.reconcile(&mut queue, now()).unwrap().in_sync);
    assert_eq!(controller.health(), HealthState::Healthy);
}

// --- Cross-cutting ---

#[test]
fn controllers_are_interchangeable_through_the_trait() {
    let mut controllers: Vec<Box<dyn Controller>> = vec![
        Box::new(ApplicationController::new(
            ApplicationId::new(),
            labrys_core::DesiredState::initial(),
            ObservedState::unknown(1),
        )),
        Box::new(ResourceController::new(postgres_resource())),
        Box::new(CapabilityController::new(
            "database.postgres",
            ResourceId::new(),
        )),
        Box::new(PreviewController::new(PreviewId::new())),
        Box::new(DeploymentController::new("web", "sha256:aaa")),
    ];
    let mut queue = JobQueue::new();
    let names: Vec<String> = controllers.iter().map(|c| c.name().to_string()).collect();
    assert_eq!(
        names,
        vec![
            "application",
            "resource",
            "capability",
            "preview",
            "deployment"
        ]
    );
    for controller in &mut controllers {
        let pass = controller.reconcile(&mut queue, now()).unwrap();
        assert!(pass.is_mismatch());
    }
    // One idempotent action per controller target.
    assert_eq!(queue.len(), 5);
}

#[test]
fn reconciliation_serializes_and_event_detail_is_safe() {
    let pass = Reconciliation {
        controller: "resource".to_string(),
        target: "res_x".to_string(),
        desired_version: 2,
        observed_version: 1,
        in_sync: false,
        drift: vec![labrys_core::controller::Drift {
            kind: DriftKind::Missing,
            detail: "gone".to_string(),
        }],
        actions: vec![IdempotencyKey::new("res_x", JobAction::Provision, 2)],
        at: now(),
    };
    let json = serde_json::to_string(&pass).unwrap();
    let back: Reconciliation = serde_json::from_str(&json).unwrap();
    assert_eq!(back, pass);
    let detail = pass.to_event_detail();
    assert!(detail.contains("MISMATCH"));
    assert!(!detail.contains("hunter2"));
}

#[test]
fn redact_reason_replaces_every_known_secret() {
    let out = redact_reason(
        "connect with password hunter2 and token tok_live_9 to db",
        &["hunter2", "tok_live_9"],
    );
    assert_eq!(
        out,
        "connect with password [redacted] and token [redacted] to db"
    );
    assert_eq!(redact_reason("clean", &[]), "clean");
    assert_eq!(redact_reason("clean", &[""]), "clean");
}
