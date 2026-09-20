use std::str::FromStr;

use chrono::{TimeZone, Utc};

use labrys_core::inspector::ExplicitApproval;
use labrys_core::{
    attach_domain, build_image, execute_rollback, plan_rollback, select_rollback_target,
    ApplicationId, BuildResult, Deployment, DeploymentId, DeploymentPhase, Domain, DomainId,
    Endpoint, EnvironmentId, HealthStatus, ImageSource, ResourceId, Revision, RollbackKind,
    Rollout, RuntimeConfig, RuntimeKind, RuntimeProfile, SandboxLimits, SupportTier,
    CONFIGURATION_DATA_WARNING, DATABASE_DATA_WARNING,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn prod_config(kind: RuntimeKind) -> RuntimeConfig {
    RuntimeConfig {
        kind,
        tier: if kind == RuntimeKind::Generic {
            SupportTier::Tier0
        } else {
            SupportTier::Tier1
        },
        profile: RuntimeProfile::Production,
        port: 8080,
        healthcheck_path: Some("/healthz".to_string()),
        dockerfile: if kind == RuntimeKind::Generic {
            Some("Dockerfile".to_string())
        } else {
            None
        },
        run_command: vec!["serve".to_string()],
        oci_image: Some(format!("registry.local/{}-app", kind.canonical_name())),
        limits: SandboxLimits::docker_default(),
    }
}

fn dev_config() -> RuntimeConfig {
    RuntimeConfig {
        profile: RuntimeProfile::Development,
        oci_image: None,
        ..prod_config(RuntimeKind::Node)
    }
}

fn revision(sha: &str) -> Revision {
    Revision::new(sha, format!("cfg-{sha}"), Some("cap-hash".to_string()))
}

fn new_deployment(kind: RuntimeKind, sha: &str) -> Deployment {
    Deployment::new(
        ApplicationId::new(),
        EnvironmentId::new(),
        revision(sha),
        prod_config(kind),
        vec![ResourceId::new()],
        None,
        now(),
    )
    .unwrap()
}

fn promoted_deployment(kind: RuntimeKind, sha: &str) -> Deployment {
    let mut d = new_deployment(kind, sha);
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("built"), &[], now())
        .unwrap();
    d.attach_endpoint(Endpoint::loopback(8080, true).unwrap())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    d
}

// --- Requirement: production deployment is a first-class object ---

#[test]
fn deployment_retains_every_required_metadata_field() {
    let deployment = promoted_deployment(RuntimeKind::Node, "abc123");

    // revision, build, image, runtime, environment, resources, endpoint,
    // health, logs, and rollback-target metadata are all present.
    assert_eq!(deployment.revision.source_sha, "abc123");
    assert!(deployment.build.as_ref().unwrap().is_success());
    assert!(deployment.image.as_ref().unwrap().is_addressable());
    assert_eq!(deployment.runtime.kind, RuntimeKind::Node);
    assert_eq!(deployment.runtime.profile, RuntimeProfile::Production);
    assert!(!deployment.resources.is_empty());
    assert!(deployment.endpoint.is_some());
    assert!(deployment.health.is_healthy());
    assert!(!deployment.logs.is_empty());
    assert!(deployment.rollback_target.is_none());

    // The record round-trips through canonical JSON.
    let json = deployment.to_json().unwrap();
    let back = Deployment::from_json(&json).unwrap();
    assert_eq!(back, deployment);
}

#[test]
fn container_only_project_produces_a_language_independent_oci_deployment() {
    // WHEN a project is buildable only through the generic container runtime.
    let mut deployment = new_deployment(RuntimeKind::Generic, "dead123");
    deployment.start_build(now()).unwrap();
    let image = build_image(
        &deployment.runtime,
        &BuildResult::succeeded("docker build ok"),
        &deployment.revision,
        &[],
    )
    .unwrap();

    // THEN it produces an OCI deployment ...
    assert_eq!(image.source, ImageSource::Generic);
    assert!(image.digest.starts_with("sha256:"));
    deployment
        .record_build(&BuildResult::succeeded("docker build ok"), &[], now())
        .unwrap();
    assert_eq!(deployment.phase, DeploymentPhase::Deploying);
    assert_eq!(
        deployment.image.as_ref().unwrap().source,
        ImageSource::Generic
    );

    // AND the record shape is identical to a native-runtime deployment.
    let native = build_image(
        &prod_config(RuntimeKind::Rust),
        &BuildResult::succeeded("cargo build ok"),
        &revision("beef00"),
        &[],
    )
    .unwrap();
    assert_eq!(native.source, ImageSource::Native);
    assert_eq!(
        std::mem::size_of_val(&image),
        std::mem::size_of_val(&native)
    );
    assert!(image.reference.ends_with(&image.digest));
}

#[test]
fn development_never_mints_an_oci_image() {
    let err = build_image(
        &dev_config(),
        &BuildResult::succeeded("dev build"),
        &revision("abc123"),
        &[],
    )
    .unwrap_err();
    assert!(matches!(err, labrys_core::CoreError::Deployment(_)));
}

#[test]
fn failed_or_cancelled_builds_never_produce_an_image() {
    let err = build_image(
        &prod_config(RuntimeKind::Node),
        &BuildResult::timed_out(600, 601),
        &revision("abc123"),
        &[],
    )
    .unwrap_err();
    assert!(
        format!("{err}").contains("timeout_secs=600") || format!("{err}").contains("Cancelled")
    );

    // A runtime with no OCI reference (Expo Go) is rejected.
    let expo = RuntimeConfig {
        kind: RuntimeKind::Expo,
        oci_image: None,
        ..prod_config(RuntimeKind::Expo)
    };
    assert!(build_image(
        &expo,
        &BuildResult::succeeded("ok"),
        &revision("abc123"),
        &[]
    )
    .is_err());
}

#[test]
fn build_failure_fails_the_deployment_with_redacted_limit_and_recovery_detail() {
    let mut deployment = new_deployment(RuntimeKind::Node, "abc123");
    deployment.start_build(now()).unwrap();

    // A secret-bearing build failure reaches logs as redacted text.
    let build = BuildResult::limit_cancelled("memory_mb=512", "oom while compiling hunter2");
    deployment
        .record_build(&build, &["hunter2"], now())
        .unwrap();

    assert_eq!(deployment.phase, DeploymentPhase::Failed);
    assert!(deployment.image.is_none());
    let logs = deployment
        .logs
        .iter()
        .map(|l| l.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!logs.contains("hunter2"));
    assert!(logs.contains("[redacted]"));
    assert!(deployment.to_event_detail().contains("Failed"));
}

// --- Requirement: health gates promotion ---

#[test]
fn deployment_is_not_promoted_before_health_passes() {
    let mut deployment = new_deployment(RuntimeKind::Node, "abc123");
    deployment.start_build(now()).unwrap();
    deployment
        .record_build(&BuildResult::succeeded("built"), &[], now())
        .unwrap();

    // Built and starting, but no health signal yet: never serves traffic.
    assert_eq!(deployment.phase, DeploymentPhase::Deploying);
    assert!(!deployment.is_promoted());
    deployment
        .record_health(HealthStatus::Starting, &[], now())
        .unwrap();
    assert!(!deployment.is_promoted());
    deployment
        .record_health(
            HealthStatus::Unknown {
                reason: "probe pending".to_string(),
            },
            &[],
            now(),
        )
        .unwrap();
    assert!(!deployment.is_promoted());

    // Only a platform Healthy observation promotes it.
    deployment
        .record_health(HealthStatus::Healthy, &[], now())
        .unwrap();
    assert!(deployment.is_promoted());
    assert!(deployment.phase.serves_traffic());
}

#[test]
fn unhealthy_new_revision_stops_promotion_and_keeps_the_rollback_target() {
    let mut rollout = Rollout::new();
    let app = ApplicationId::new();
    let env = EnvironmentId::new();

    // An earlier revision is serving production.
    let first = rollout
        .begin(
            app,
            env,
            revision("v1"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    rollout.get_mut(first).unwrap().start_build(now()).unwrap();
    rollout
        .get_mut(first)
        .unwrap()
        .record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    rollout
        .get_mut(first)
        .unwrap()
        .record_health(HealthStatus::Healthy, &[], now())
        .unwrap();
    rollout.promote(first, now()).unwrap();

    // WHEN a new revision is rolled out ...
    let second = rollout
        .begin(
            app,
            env,
            revision("v2"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    // ... it inherits the prior healthy deployment as its rollback target.
    assert_eq!(rollout.get(second).unwrap().rollback_target, Some(first));

    let d = rollout.get_mut(second).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();

    // ... and its health checks fail.
    d.record_health(
        HealthStatus::Unhealthy {
            reason: "/healthz answered 503".to_string(),
        },
        &[],
        now(),
    )
    .unwrap();
    assert_eq!(rollout.get(second).unwrap().phase, DeploymentPhase::Failed);
    assert!(!rollout.get(second).unwrap().is_promoted());

    // THEN promotion refuses to route traffic to it ...
    assert!(rollout.promote(second, now()).is_err());
    assert_eq!(rollout.current(), Some(first));
    assert_eq!(
        rollout.current_digest().unwrap(),
        rollout.get(first).unwrap().image.as_ref().unwrap().digest
    );

    // ... AND the prior healthy rollback target remains identifiable.
    assert_eq!(rollout.rollback_target(), Some(first));
    assert_eq!(select_rollback_target(&rollout, second), Some(first));
    assert_eq!(rollout.get(first).unwrap().phase, DeploymentPhase::Healthy);
}

#[test]
fn promotion_supersedes_the_previous_deployment() {
    let mut rollout = Rollout::new();
    let app = ApplicationId::new();
    let env = EnvironmentId::new();
    let first = rollout
        .begin(
            app,
            env,
            revision("v1"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(first).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(first, now()).unwrap();

    let second = rollout
        .begin(
            app,
            env,
            revision("v2"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(second).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(second, now()).unwrap();

    assert_eq!(rollout.current(), Some(second));
    assert_eq!(
        rollout.get(first).unwrap().phase,
        DeploymentPhase::Superseded
    );
    // A superseded deployment is still a valid rollback target.
    assert_eq!(select_rollback_target(&rollout, second), Some(first));
}

// --- Rollback boundaries ---

#[test]
fn deployment_rollback_warns_that_database_data_is_not_restored() {
    let mut rollout = Rollout::new();
    let app = ApplicationId::new();
    let env = EnvironmentId::new();

    let first = rollout
        .begin(
            app,
            env,
            revision("v1"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(first).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(first, now()).unwrap();

    // WHEN a revision introduces a migration and is promoted ...
    let second = rollout
        .begin(
            app,
            env,
            revision("v2").with_migration(true),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(second).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(second, now()).unwrap();

    let mut domain = Domain::new(env, "app.example.com", true);
    let approval = ExplicitApproval::granted("bob", "attach domain");
    let d = rollout.get(second).unwrap();
    attach_domain(&mut domain, d, &approval).unwrap();
    assert_eq!(domain.deployment, Some(second));

    // ... THEN rolling back the application image warns about database data.
    let from = rollout.get(second).unwrap().clone();
    let to = rollout.get(first).unwrap().clone();
    let plan = plan_rollback(RollbackKind::Deployment, &from, &to).unwrap();
    assert!(plan.warnings.iter().any(|w| w == DATABASE_DATA_WARNING));
    assert!(plan.migration_in_delta);
    assert!(plan.warnings.iter().any(|w| w.contains("migration")));
    assert!(plan.requires_approval);

    // An unapproved rollback is refused.
    let denied = ExplicitApproval::denied("agent", "rollback");
    let mut domains = vec![domain.clone()];
    assert!(matches!(
        execute_rollback(&mut rollout, &plan, &denied, &mut domains, now()).unwrap_err(),
        labrys_core::CoreError::ApprovalRequired(_)
    ));
    assert_eq!(rollout.current(), Some(second));

    // With human approval traffic moves and the domain follows.
    let outcome = execute_rollback(&mut rollout, &plan, &approval, &mut domains, now()).unwrap();
    assert!(outcome.traffic_moved);
    assert!(outcome
        .detail
        .contains("database data is not automatically rolled back"));
    assert_eq!(rollout.current(), Some(first));
    assert_eq!(
        rollout.get(second).unwrap().phase,
        DeploymentPhase::RolledBack
    );
    assert_eq!(rollout.get(first).unwrap().phase, DeploymentPhase::Healthy);
    assert_eq!(domains[0].deployment, Some(first));
}

#[test]
fn source_and_configuration_rollback_are_independent_operations() {
    let mut rollout = Rollout::new();
    let app = ApplicationId::new();
    let env = EnvironmentId::new();
    let first = rollout
        .begin(
            app,
            env,
            revision("v1"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(first).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(first, now()).unwrap();
    let second = rollout
        .begin(
            app,
            env,
            revision("v2"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(second).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(second, now()).unwrap();

    let from = rollout.get(second).unwrap().clone();
    let to = rollout.get(first).unwrap().clone();
    let approval = ExplicitApproval::granted("bob", "rollback");

    // A configuration rollback never claims to move traffic or the image.
    let config_plan = plan_rollback(RollbackKind::Configuration, &from, &to).unwrap();
    assert!(config_plan
        .warnings
        .iter()
        .any(|w| w == CONFIGURATION_DATA_WARNING));
    let outcome = execute_rollback(&mut rollout, &config_plan, &approval, &mut [], now()).unwrap();
    assert!(!outcome.traffic_moved);
    assert_eq!(rollout.current(), Some(second));
    assert_eq!(rollout.get(second).unwrap().phase, DeploymentPhase::Healthy);

    // A source rollback is its own operation, distinct from both.
    let source_plan = plan_rollback(RollbackKind::Source, &from, &to).unwrap();
    assert!(!source_plan
        .warnings
        .iter()
        .any(|w| w == DATABASE_DATA_WARNING));
    assert!(source_plan.warnings.iter().any(|w| w.contains("rebuild")));
    assert_ne!(source_plan.kind, config_plan.kind);
}

#[test]
fn rollback_target_selection_never_picks_unbuilt_or_failed_revisions() {
    let mut rollout = Rollout::new();
    let app = ApplicationId::new();
    let env = EnvironmentId::new();

    let first = rollout
        .begin(
            app,
            env,
            revision("v1"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(first).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    rollout.promote(first, now()).unwrap();

    // A revision that never built cannot be a target.
    let unbuilt = rollout
        .begin(
            app,
            env,
            revision("v2"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    assert_eq!(select_rollback_target(&rollout, unbuilt), Some(first));

    // A failed revision cannot be a target either.
    let failed = rollout
        .begin(
            app,
            env,
            revision("v3"),
            prod_config(RuntimeKind::Node),
            vec![],
            now(),
        )
        .unwrap();
    let d = rollout.get_mut(failed).unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::failed("compile error"), &[], now())
        .unwrap();
    assert_eq!(rollout.get(failed).unwrap().phase, DeploymentPhase::Failed);
    assert_eq!(select_rollback_target(&rollout, failed), Some(first));

    // Rolling back to an unbuilt deployment is rejected by the plan.
    let from = rollout.get(failed).unwrap().clone();
    let to = rollout.get(unbuilt).unwrap().clone();
    assert!(plan_rollback(RollbackKind::Deployment, &from, &to).is_err());
    // Rolling back onto itself is rejected.
    let same = rollout.get(first).unwrap().clone();
    assert!(plan_rollback(RollbackKind::Deployment, &same, &same).is_err());
}

// --- Domain change approval ---

#[test]
fn domain_changes_require_approval_and_a_promoted_target() {
    let env = EnvironmentId::new();
    let mut domain = Domain::new(env, "app.example.com", true);
    assert!(domain.is_routable());
    assert!(!Domain::new(env, "not a host", true).is_routable());

    // Unpromoted deployment: even with approval, no routing.
    let pending = new_deployment(RuntimeKind::Node, "abc123");
    let approval = ExplicitApproval::granted("bob", "attach domain");
    assert!(matches!(
        attach_domain(&mut domain, &pending, &approval).unwrap_err(),
        labrys_core::CoreError::Deployment(_)
    ));
    assert_eq!(domain.deployment, None);

    // Promoted deployment but no approval: refused.
    let promoted = promoted_deployment(RuntimeKind::Node, "abc123");
    let denied = ExplicitApproval::denied("agent", "attach domain");
    assert!(matches!(
        attach_domain(&mut domain, &promoted, &denied).unwrap_err(),
        labrys_core::CoreError::ApprovalRequired(_)
    ));
    let anonymous = ExplicitApproval {
        approved: true,
        approver: " ".to_string(),
        proposal_summary: "attach".to_string(),
    };
    assert!(attach_domain(&mut domain, &promoted, &anonymous).is_err());

    // Granted, named approval routes traffic to the verified revision.
    attach_domain(&mut domain, &promoted, &approval).unwrap();
    assert_eq!(domain.deployment, Some(promoted.id));
}

#[test]
fn deployment_phase_graph_is_validated_and_evidenced() {
    // Illegal jumps are rejected.
    let mut d = new_deployment(RuntimeKind::Node, "abc123");
    assert!(d.record_health(HealthStatus::Healthy, &[], now()).is_err()); // Pending -> Healthy skips the gate
    assert!(!DeploymentPhase::Pending.can_transition_to(DeploymentPhase::Healthy));
    assert!(DeploymentPhase::RolledBack.is_terminal());

    // The happy path records evidence at every step.
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    d.mark_superseded(now()).unwrap();
    d.mark_rolled_back("rollback to v1", now()).unwrap();
    assert_eq!(d.phase, DeploymentPhase::RolledBack);
    assert!(DeploymentPhase::RolledBack.is_terminal());
    let path: Vec<DeploymentPhase> = d.transitions.iter().map(|t| t.to).collect();
    assert_eq!(
        path,
        vec![
            DeploymentPhase::Pending,
            DeploymentPhase::Building,
            DeploymentPhase::Deploying,
            DeploymentPhase::Healthy,
            DeploymentPhase::Superseded,
            DeploymentPhase::RolledBack,
        ]
    );
    // Same-state no-ops are rejected.
    assert!(d.mark_rolled_back("again", now()).is_err());
}

#[test]
fn endpoint_attachment_requires_an_exposed_port() {
    let mut d = new_deployment(RuntimeKind::Node, "abc123");
    let hidden = Endpoint::loopback(8080, false).unwrap();
    assert!(d.attach_endpoint(hidden).is_err());
    d.attach_endpoint(Endpoint::loopback(8080, true).unwrap())
        .unwrap();
    assert!(d.endpoint.is_some());
    assert!(DeploymentId::new() != DeploymentId::new());
}

#[test]
fn digests_are_deterministic_per_revision() {
    let config = prod_config(RuntimeKind::Node);
    let build = BuildResult::succeeded("ok");
    let a = build_image(&config, &build, &revision("abc123"), &[]).unwrap();
    let b = build_image(&config, &build, &revision("abc123"), &[]).unwrap();
    let c = build_image(&config, &build, &revision("def456"), &[]).unwrap();
    assert_eq!(a.digest, b.digest);
    assert_ne!(a.digest, c.digest);
}

#[test]
fn deployment_event_detail_carries_no_secret_values() {
    let mut d = new_deployment(RuntimeKind::Node, "abc123");
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("ok"), &[], now())
        .unwrap();
    d.record_health(
        HealthStatus::Unhealthy {
            reason: "auth failed for token tok_live_9".to_string(),
        },
        &["tok_live_9"],
        now(),
    )
    .unwrap();
    let detail = d.to_event_detail();
    assert!(!detail.contains("tok_live_9"));
    assert!(detail.contains("Failed"));
    let last = d.transitions.last().unwrap();
    assert!(!last
        .reason
        .clone()
        .unwrap_or_default()
        .contains("tok_live_9"));
}

#[test]
fn contract_version_is_exposed() {
    assert_eq!(labrys_core::DEPLOYMENT_CONTRACT_VERSION, 1);
    assert!(DeploymentId::from_str(&DeploymentId::new().to_string()).is_ok());
    assert!(DomainId::from_str(&DomainId::new().to_string()).is_ok());
    assert!(DeploymentId::from_str("dep_nope").is_err());
}
