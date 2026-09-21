use chrono::{TimeZone, Utc};

use labrys_core::{
    gate_registry_push, note_agent_claim_ignored, plan_traffic_attachment, sanitize_failure,
    verify_registry_digest, ApplicationId, BuildResult, CertificateState, Deployment,
    DeploymentPhase, DnsState, Domain, DomainDelivery, Endpoint, EnvironmentId, ExplicitApproval,
    HealthStatus, ProviderAction, ProviderKind, ProviderObservation, ProviderOperation,
    RegistryArtifact, ResourceId, ResourcePhase, Revision, RuntimeConfig, RuntimeKind,
    RuntimeProfile, SandboxLimits, SupportTier, CERTIFICATE_FAILURE_RECOVERY,
    DIGEST_MISMATCH_RECOVERY, PROVIDER_FAILURE_RECOVERY,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn operation(action: ProviderAction) -> ProviderOperation {
    ProviderOperation::plan(
        ApplicationId::new(),
        Some(EnvironmentId::new()),
        ResourceId::new(),
        "database.postgres",
        "postgres.local",
        ProviderKind::Postgres,
        action,
        "trace-1",
    )
    .unwrap()
}

fn approval_for(op: &ProviderOperation) -> ExplicitApproval {
    ExplicitApproval::granted("ada", format!("{} {}", op.resource_id, op.capability))
}

fn prod_config() -> RuntimeConfig {
    RuntimeConfig {
        kind: RuntimeKind::Node,
        tier: SupportTier::Tier1,
        profile: RuntimeProfile::Production,
        port: 8080,
        healthcheck_path: Some("/healthz".to_string()),
        dockerfile: None,
        run_command: vec!["serve".to_string()],
        oci_image: Some("registry.local/app".to_string()),
        limits: SandboxLimits::docker_default(),
    }
}

fn promoted_deployment() -> Deployment {
    let mut d = Deployment::new(
        ApplicationId::new(),
        EnvironmentId::new(),
        Revision::new("abc123", "cfg-abc123", Some("cap-hash".to_string())),
        prod_config(),
        vec![ResourceId::new()],
        None,
        now(),
    )
    .unwrap();
    d.start_build(now()).unwrap();
    d.record_build(&BuildResult::succeeded("built"), &[], now())
        .unwrap();
    d.attach_endpoint(Endpoint::loopback(8080, true).unwrap())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], now()).unwrap();
    assert_eq!(d.phase, DeploymentPhase::Healthy);
    d
}

// --- Requirement: provider operations are scoped and observable ---

#[test]
fn capability_provisioning_is_accepted_but_not_ready() {
    // WHEN a provider accepts a PostgreSQL provisioning request ...
    let observation = ProviderObservation::accepted("postgres.local accepted");

    // THEN the resource remains provisioning ...
    assert_eq!(
        observation.clone().target_phase(),
        ResourcePhase::Provisioning
    );

    // AND only a later platform observation can make it ready.
    let mut claims_ignored = 0;
    note_agent_claim_ignored(&mut claims_ignored);
    assert_eq!(claims_ignored, 1);
    assert_eq!(
        ProviderObservation::Ready {
            detail: "platform probe: ready".to_string()
        }
        .target_phase(),
        ResourcePhase::Ready
    );
}

#[test]
fn provider_error_with_credential_is_redacted_with_recovery() {
    let secret = "super-secret-pw-123";
    // WHEN a provider error includes a credential ...
    let observation = ProviderObservation::failed(
        &format!("connection failed with password {secret}"),
        &[secret],
    );

    // THEN persisted events and diagnostics redact it ...
    let detail = observation.to_event_detail();
    assert!(!detail.contains(secret));
    assert!(detail.contains("[redacted]"));

    // AND the operation exposes recovery guidance.
    let (reason, recovery) = sanitize_failure(
        &format!("connection failed with password {secret}"),
        &[secret],
    );
    assert!(!reason.contains(secret));
    assert_eq!(recovery, PROVIDER_FAILURE_RECOVERY);
    match observation {
        ProviderObservation::Failed { recovery: r, .. } => assert_eq!(r, PROVIDER_FAILURE_RECOVERY),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn provider_mismatch_is_rejected_at_plan_time() {
    let err = ProviderOperation::plan(
        ApplicationId::new(),
        None,
        ResourceId::new(),
        "database.postgres",
        "auth.local",
        ProviderKind::Auth,
        ProviderAction::Provision,
        "trace-1",
    )
    .unwrap_err();
    assert!(err.to_string().contains("does not supply"));
}

#[test]
fn duplicate_requests_share_one_idempotency_key() {
    let resource = ResourceId::new();
    let first = ProviderOperation::plan(
        ApplicationId::new(),
        None,
        resource,
        "database.postgres",
        "postgres.local",
        ProviderKind::Postgres,
        ProviderAction::Provision,
        "trace-1",
    )
    .unwrap();
    let retry = first.clone().with_attempt(2);
    assert_eq!(first.idempotency_key, retry.idempotency_key);
}

// --- Requirement: destructive provider actions require approval ---

#[test]
fn agent_database_deletion_without_approval_makes_no_provider_call() {
    let op = operation(ProviderAction::Delete);

    // WHEN an agent requests deletion without approval ...
    let err = op.require_approval(None).unwrap_err();

    // THEN no provider call occurs (the gate errors before dispatch) ...
    assert!(err
        .to_string()
        .contains("requires a granted, named human approval"));

    // AND the denial is attributable and recoverable.
    let denied = ExplicitApproval::denied("ada", op.resource_id.to_string());
    let err = op.require_approval(Some(&denied)).unwrap_err();
    assert!(err.to_string().contains("no provider call occurred"));

    // A granted, named approval scoped to the resource succeeds.
    op.require_approval(Some(&approval_for(&op))).unwrap();

    // A granted approval for an unrelated scope still blocks.
    let wrong = ExplicitApproval::granted("ada", "unrelated scope");
    let err = op.require_approval(Some(&wrong)).unwrap_err();
    assert!(err.to_string().contains("does not match"));
}

#[test]
fn non_destructive_actions_need_no_approval() {
    operation(ProviderAction::Provision)
        .require_approval(None)
        .unwrap();
    operation(ProviderAction::ReadinessCheck)
        .require_approval(None)
        .unwrap();
}

// --- Requirement: production artifacts are digest verified ---

#[test]
fn registry_push_requires_verified_production_artifact() {
    let verified = RegistryArtifact::new("registry.local/app", "sha256:abc", "abc123", true);
    gate_registry_push(&verified).unwrap();

    let unverified = RegistryArtifact::new("registry.local/app", "sha256:abc", "abc123", false);
    assert!(gate_registry_push(&unverified).is_err());

    let bare = RegistryArtifact::new("registry.local/app", "not-a-digest", "abc123", true);
    assert!(gate_registry_push(&bare).is_err());
}

#[test]
fn registry_digest_mismatch_blocks_promotion_with_recovery() {
    verify_registry_digest("sha256:aaa", "sha256:aaa").unwrap();

    // WHEN a push returns a digest different from the build artifact ...
    let err = verify_registry_digest("sha256:aaa", "sha256:bbb").unwrap_err();

    // THEN deployment remains unpromoted (caller keeps it deploying) AND the
    // mismatch is recorded with recovery guidance.
    assert!(err.to_string().contains("digest mismatch"));
    assert!(err.to_string().contains(DIGEST_MISMATCH_RECOVERY));
}

// --- Requirement: domains route only to promoted healthy deployments ---

#[test]
fn traffic_requires_dns_tls_approval_and_promoted_target() {
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "app.example.com", true);
    let approval = ExplicitApproval::granted("ada", "route app.example.com");

    let mut delivery = DomainDelivery::new(domain.id);
    delivery.dns = DnsState::Propagated;
    delivery.tls = CertificateState::Issued;
    plan_traffic_attachment(&mut delivery, &domain, &deployment, &approval).unwrap();
    assert!(delivery.traffic_attached);
    assert!(delivery.is_healthy());

    // Unpromoted deployments never receive traffic.
    let mut pending = Deployment::new(
        ApplicationId::new(),
        EnvironmentId::new(),
        Revision::new("zzz", "cfg-zzz", None),
        prod_config(),
        vec![],
        None,
        now(),
    )
    .unwrap();
    let _ = &mut pending;
    let mut delivery = DomainDelivery::new(domain.id);
    delivery.dns = DnsState::Propagated;
    delivery.tls = CertificateState::Issued;
    let err = plan_traffic_attachment(&mut delivery, &domain, &pending, &approval).unwrap_err();
    assert!(err.to_string().contains("unpromoted"));
    assert!(!delivery.traffic_attached);
}

#[test]
fn certificate_failure_reports_failure_not_healthy() {
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "app.example.com", true);
    let approval = ExplicitApproval::granted("ada", "route app.example.com");

    let mut delivery = DomainDelivery::new(domain.id);
    delivery.dns = DnsState::Propagated;
    // WHEN TLS issuance fails for an approved domain ...
    delivery.report_certificate_failure("issuer refused tls-secret-xyz", &["tls-secret-xyz"]);

    // THEN traffic is not attached ...
    assert!(!delivery.traffic_attached);
    assert!(!delivery.is_healthy());

    // AND the domain reports the certificate failure rather than healthy.
    let detail = delivery.to_event_detail();
    assert!(!detail.contains("tls-secret-xyz"));
    assert!(detail.contains("certificate issuance failed"));
    assert!(detail.contains(CERTIFICATE_FAILURE_RECOVERY));

    let err = plan_traffic_attachment(&mut delivery, &domain, &deployment, &approval).unwrap_err();
    assert!(err.to_string().contains("certificate issuance failed"));
}

// --- Cross-surface: rollback boundary still holds ---

#[test]
fn provider_delivery_never_claims_database_restoration() {
    let text = labrys_core::DATABASE_DATA_WARNING.to_string();
    assert!(text.contains("database data is not"));
}
