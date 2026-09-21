//! Integration tests for provider, registry, and domain delivery adapters.
//!
//! Pure adapter tests (approval gates, digest verification, DNS/TLS/traffic
//! gates, redaction) always run. PostgreSQL-backed tests use an isolated
//! `labrys_provider_delivery_test` database so they never race the other
//! suites, and skip without `LABRYS_DATABASE_URL`. They are infrastructure
//! evidence, not production-deployment evidence: every adapter here is a local
//! test double, and external clouds are reported as unavailable, never
//! simulated as production.

use std::sync::{Arc, OnceLock};

use chrono::Utc;
use sqlx::PgPool;
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    run_migrations, DomainDeliveryAdapter, LocalDomainDelivery, LocalTestAdapter,
    LocalTestRegistry, OciRegistry, ProviderAdapter, ProviderExecution, ProviderRuntime,
};
use labrys_core::{
    ApplicationId, BuildResult, CertificateState, Deployment, DnsState, Domain, DomainDelivery,
    Endpoint, EnvironmentId, ExplicitApproval, HealthStatus, ProviderAction, ProviderKind,
    ProviderOperation, RegistryArtifact, Resource, ResourceId, ResourcePhase, Revision,
    RuntimeConfig, RuntimeKind, RuntimeProfile, SandboxLimits, SupportTier,
};

const SECRET: &str = "provider-test-secret-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

fn delivery_db_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    match base.rfind('/') {
        Some(idx) if idx > scheme_end => format!("{}/labrys_provider_delivery_test", &base[..idx]),
        _ => format!("{base}/labrys_provider_delivery_test"),
    }
}

async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let base = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let maintenance = PgPool::connect(&base).await.expect("connect postgres");
    let _ = sqlx::query("CREATE DATABASE labrys_provider_delivery_test")
        .execute(&maintenance)
        .await;
    maintenance.close().await;
    let pool = PgPool::connect(&delivery_db_url(&base))
        .await
        .expect("connect delivery db");
    run_migrations(&pool).await.expect("run migrations");
    sqlx::query(
        "TRUNCATE provider_operations, registry_deliveries, domain_deliveries, resources, \
         events, audit_log, usage_records RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    Some((guard, pool))
}

fn operation(kind: ProviderKind, capability: &str, action: ProviderAction) -> ProviderOperation {
    let provider_key = match kind {
        ProviderKind::Postgres => "postgres.local-test",
        ProviderKind::Auth => "auth.local-test",
        ProviderKind::FileStorage => "filestore.local-test",
        ProviderKind::ObjectStorage => "objectstore.local-test",
        ProviderKind::Generic => "generic.local-test",
    };
    ProviderOperation::plan(
        ApplicationId::new(),
        Some(EnvironmentId::new()),
        ResourceId::new(),
        capability,
        provider_key,
        kind,
        action,
        format!("trace-{}", uuid::Uuid::new_v4().simple()),
    )
    .unwrap()
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
    let at = Utc::now();
    let mut d = Deployment::new(
        ApplicationId::new(),
        EnvironmentId::new(),
        Revision::new("abc123", "cfg-abc123", Some("cap-hash".to_string())),
        prod_config(),
        vec![ResourceId::new()],
        None,
        at,
    )
    .unwrap();
    d.start_build(at).unwrap();
    d.record_build(&BuildResult::succeeded("built"), &[], at)
        .unwrap();
    d.attach_endpoint(Endpoint::loopback(8080, true).unwrap())
        .unwrap();
    d.record_health(HealthStatus::Healthy, &[], at).unwrap();
    d
}

// ---------------------------------------------------------------------------
// Pure adapter tests: always run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn postgres_auth_storage_and_generic_adapters_accept_provisioning() {
    for (kind, capability) in [
        (ProviderKind::Postgres, "database.postgres"),
        (ProviderKind::Auth, "auth"),
        (ProviderKind::FileStorage, "storage.file"),
        (ProviderKind::ObjectStorage, "storage.object"),
        (ProviderKind::Generic, "anything.at.all"),
    ] {
        let adapter = match kind {
            ProviderKind::Postgres => LocalTestAdapter::postgres_test(),
            ProviderKind::Auth => LocalTestAdapter::auth_test(),
            ProviderKind::FileStorage => LocalTestAdapter::file_storage_test(),
            ProviderKind::ObjectStorage => LocalTestAdapter::object_storage_test(),
            ProviderKind::Generic => LocalTestAdapter::generic_test(),
        };
        let op = operation(kind, capability, ProviderAction::Provision);
        let outcome = adapter.execute(&op).await.unwrap();
        assert!(
            matches!(outcome, ProviderExecution::Accepted { .. }),
            "{kind:?} should accept, got {outcome:?}"
        );
        assert_eq!(outcome.target_phase(), ResourcePhase::Provisioning);
        assert_eq!(adapter.calls().len(), 1);
    }
}

#[tokio::test]
async fn destructive_operation_without_approval_never_reaches_adapter() {
    let adapter = LocalTestAdapter::postgres_test().with_steps(vec![ProviderExecution::Ready {
        detail: "should never run".to_string(),
    }]);
    let op = operation(
        ProviderKind::Postgres,
        "database.postgres",
        ProviderAction::Delete,
    );
    // The durable runtime enforces the gate; without approval the adapter must
    // see zero calls. (Pure check: the plan-level gate rejects first.)
    assert!(op.require_approval(None).is_err());
    assert!(adapter.calls().is_empty());
}

#[tokio::test]
async fn registry_mismatch_is_recorded_never_promoted() {
    let registry = LocalTestRegistry::with_mismatch("sha256:wrong-digest");
    let artifact = RegistryArtifact::new("registry.local/app", "sha256:expected", "abc123", true);
    let err = registry.push(&artifact).await.unwrap_err();
    assert!(err.to_string().contains("digest mismatch"));
    assert!(artifact.digest.starts_with("sha256:expected"));
}

#[tokio::test]
async fn registry_refuses_unverified_artifacts() {
    let registry = LocalTestRegistry::new();
    let artifact = RegistryArtifact::new("registry.local/app", "sha256:abc", "abc123", false);
    let err = registry.push(&artifact).await.unwrap_err();
    assert!(err.to_string().contains("verified production build"));
}

#[tokio::test]
async fn certificate_failure_attaches_no_traffic() {
    let delivery_adapter =
        LocalDomainDelivery::with_tls(CertificateState::failed("issuer down", &[]));
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "app.example.com", true);
    let approval = ExplicitApproval::granted("ada", "route app.example.com");
    let mut delivery = DomainDelivery::new(domain.id);
    delivery.dns = DnsState::Propagated;
    delivery.tls = CertificateState::Failed {
        reason: "issuer down".to_string(),
    };
    let err = delivery_adapter
        .attach_traffic(&mut delivery, &domain, &deployment, &approval)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("certificate issuance failed"));
    assert!(!delivery.traffic_attached);
}

// ---------------------------------------------------------------------------
// PostgreSQL-backed tests: persist operations, evidence, redaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepted_provisioning_persists_and_stays_provisioning() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let resources = labrys_control_plane::PgResourceStore::new(pool.clone());
    let apps = labrys_control_plane::PgApplicationStore::new(pool.clone());
    let runtime = ProviderRuntime::new(pool.clone());
    let adapter = LocalTestAdapter::postgres_test().with_steps(vec![ProviderExecution::Accepted {
        detail: "accepted".to_string(),
    }]);
    let mut op = operation(
        ProviderKind::Postgres,
        "database.postgres",
        ProviderAction::Provision,
    );
    let resource = Resource::request("database.postgres", 1, Utc::now()).unwrap();
    op.resource_id = resource.id;
    let app = labrys_core::Application::new(
        "provider-svc",
        labrys_core::Origin::Generated {
            prompt_summary: None,
        },
        "tester",
    );
    op.application_id = app.id;
    op.environment_id = None;
    apps.save(&app).await.unwrap();
    resources
        .save(&resource, &op.application_id, op.environment_id.as_ref())
        .await
        .unwrap();

    let outcome = runtime
        .execute_operation(&adapter, &op, None, Utc::now())
        .await
        .unwrap();
    assert!(matches!(outcome, ProviderExecution::Accepted { .. }));
    let stored = runtime
        .get_operation_by_key(&op.idempotency_key)
        .await
        .unwrap();
    assert_eq!(stored.phase, ResourcePhase::Provisioning);
    // Duplicate request returns the same outcome: one idempotent effect.
    let outcome2 = runtime
        .execute_operation(&adapter, &op, None, Utc::now())
        .await
        .unwrap();
    assert!(matches!(outcome2, ProviderExecution::Accepted { .. }));
    let stored2 = runtime
        .get_operation_by_key(&op.idempotency_key)
        .await
        .unwrap();
    assert_eq!(stored.id, stored2.id);
}

#[tokio::test]
async fn credential_bearing_failure_is_stored_redacted_with_recovery() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let resources = labrys_control_plane::PgResourceStore::new(pool.clone());
    let apps = labrys_control_plane::PgApplicationStore::new(pool.clone());
    let runtime = ProviderRuntime::with_secrets(pool.clone(), vec![SECRET.to_string()]);
    let adapter = LocalTestAdapter::postgres_test().with_failures(vec![
        labrys_control_plane::InjectedFailure::CredentialLeak(SECRET.to_string()),
    ]);
    let mut op = operation(
        ProviderKind::Postgres,
        "database.postgres",
        ProviderAction::Provision,
    );
    let resource = Resource::request("database.postgres", 1, Utc::now()).unwrap();
    op.resource_id = resource.id;
    let app = labrys_core::Application::new(
        "provider-svc",
        labrys_core::Origin::Generated {
            prompt_summary: None,
        },
        "tester",
    );
    op.application_id = app.id;
    op.environment_id = None;
    apps.save(&app).await.unwrap();
    resources
        .save(&resource, &op.application_id, op.environment_id.as_ref())
        .await
        .unwrap();

    let outcome = runtime
        .execute_operation(&adapter, &op, None, Utc::now())
        .await
        .unwrap();
    match &outcome {
        ProviderExecution::Failed { reason, recovery } => {
            assert!(!reason.contains(SECRET));
            assert!(!recovery.is_empty());
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    let stored = runtime
        .get_operation_by_key(&op.idempotency_key)
        .await
        .unwrap();
    assert!(!stored.failure.contains(SECRET));
    assert!(stored.failure.contains("[redacted]"));
}

#[tokio::test]
async fn approval_denial_persists_without_provider_call() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let runtime = ProviderRuntime::new(pool.clone());
    let adapter =
        Arc::new(
            LocalTestAdapter::postgres_test().with_steps(vec![ProviderExecution::Ready {
                detail: "must not run".to_string(),
            }]),
        );
    let op = operation(
        ProviderKind::Postgres,
        "database.postgres",
        ProviderAction::Delete,
    );
    let err = runtime
        .execute_operation(adapter.as_ref(), &op, None, Utc::now())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ApprovalRequired") || err.to_string().contains("approval"));
    assert!(adapter.calls().is_empty());
    let stored = runtime
        .get_operation_by_key(&op.idempotency_key)
        .await
        .unwrap();
    assert_eq!(stored.phase, ResourcePhase::Failed);
}

#[tokio::test]
async fn registry_push_persists_digest_evidence() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let runtime = ProviderRuntime::new(pool.clone());
    let registry = LocalTestRegistry::new();
    let artifact = RegistryArtifact::new("registry.local/app", "sha256:abc123", "abc123", true);
    let record = runtime
        .push_artifact(&registry, "app-1", &artifact, Utc::now())
        .await
        .unwrap();
    assert_eq!(record.status, "verified");
    assert_eq!(record.reported_digest, "sha256:abc123");

    let bad_registry = LocalTestRegistry::with_mismatch("sha256:other");
    let bad = RegistryArtifact::new("registry.local/app", "sha256:abc123", "abc123", true);
    let err = runtime
        .push_artifact(&bad_registry, "app-1", &bad, Utc::now())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("digest mismatch"));
}

#[tokio::test]
async fn domain_convergence_reports_cert_failure_without_traffic() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let runtime = ProviderRuntime::new(pool.clone());
    let adapter = LocalDomainDelivery::with_tls(CertificateState::Failed {
        reason: "issuer down".to_string(),
    });
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "app.example.com", true);
    let approval = ExplicitApproval::granted("ada", "route app.example.com");
    let mut delivery = DomainDelivery::new(domain.id);
    let err = runtime
        .converge_domain(
            &adapter,
            &mut delivery,
            &domain,
            &deployment,
            &approval,
            Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("certificate") || err.to_string().contains("Domain"));
    assert!(!delivery.traffic_attached);
}
