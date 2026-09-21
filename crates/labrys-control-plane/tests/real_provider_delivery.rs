//! Integration tests for the real provider delivery adapters.
//!
//! Pure unit tests (reference parsing, digest vectors, DNS states, TLS
//! blocker reporting, identifier validation) always run. PostgreSQL-backed
//! tests use an isolated `labrys_real_provider_test` database and skip
//! without `LABRYS_DATABASE_URL`. Docker-backed tests start a real
//! `registry:2` container and skip when no runtime (or no image pull) is
//! available. TLS tests need the `openssl` CLI and skip without it. Skips
//! are infrastructure absence, never simulated passes: every executed path
//! performs real provisioning, pushes, issuance, and deletions.

use std::sync::{Arc, OnceLock};

use chrono::Utc;
use sqlx::{PgPool, Row};
use tokio::sync::{Mutex, MutexGuard};

use labrys_control_plane::{
    oci_manifest, run_migrations, Config, DomainDeliveryAdapter, FileStorageProvisioner,
    JobDispatcher, ObjectStorageProvisioner, OciRegistry, OciRegistryClient, OpensslTlsIssuer,
    PostgresProvisioner, ProviderAdapter, ProviderExecution, ProviderRuntime, RealDomainDelivery,
    ResolvingDns, RuntimeMode,
};
use labrys_core::{
    ApplicationId, BuildResult, Deployment, DnsState, Domain, DomainDelivery, Endpoint,
    EnvironmentId, ExplicitApproval, HealthStatus, ProviderAction, ProviderKind, ProviderOperation,
    RegistryArtifact, Resource, ResourceId, ResourcePhase, Revision, RuntimeConfig, RuntimeKind,
    RuntimeProfile, SandboxLimits, SupportTier,
};

const SECRET: &str = "real-provider-test-secret-do-not-log";

fn database_url() -> Option<String> {
    std::env::var("LABRYS_DATABASE_URL")
        .ok()
        .or_else(|| std::env::var("DATABASE_URL").ok())
}

fn delivery_db_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    match base.rfind('/') {
        Some(idx) if idx > scheme_end => format!("{}/labrys_real_provider_test", &base[..idx]),
        _ => format!("{base}/labrys_real_provider_test"),
    }
}

async fn setup() -> Option<(MutexGuard<'static, ()>, PgPool)> {
    let base = database_url()?;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().await;
    let maintenance = PgPool::connect(&base).await.expect("connect postgres");
    let _ = sqlx::query("CREATE DATABASE labrys_real_provider_test")
        .execute(&maintenance)
        .await;
    maintenance.close().await;
    let pool = PgPool::connect(&delivery_db_url(&base))
        .await
        .expect("connect real-provider db");
    run_migrations(&pool).await.expect("run migrations");
    sqlx::query(
        "TRUNCATE provider_operations, registry_deliveries, domain_deliveries, resources, \
         applications, events, audit_log, usage_records RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate");
    Some((guard, pool))
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "labrys-real-provider-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

fn provision_op(
    kind: ProviderKind,
    capability: &str,
    provider_key: &str,
    action: ProviderAction,
) -> ProviderOperation {
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

async fn save_scope(
    pool: &PgPool,
    op: &mut ProviderOperation,
) -> (labrys_core::Application, Resource) {
    let apps = labrys_control_plane::PgApplicationStore::new(pool.clone());
    let resources = labrys_control_plane::PgResourceStore::new(pool.clone());
    let app = labrys_core::Application::new(
        "real-provider-svc",
        labrys_core::Origin::Generated {
            prompt_summary: None,
        },
        "tester",
    );
    let resource = Resource::request("scope", 1, Utc::now()).unwrap();
    op.application_id = app.id;
    op.resource_id = resource.id;
    // The scope rows need no environment: the existing suite persists scope
    // the same way (environment None), keeping the environments FK intact.
    op.environment_id = None;
    apps.save(&app).await.unwrap();
    resources
        .save(&resource, &op.application_id, op.environment_id.as_ref())
        .await
        .unwrap();
    (app, resource)
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
    assert!(d.is_promoted());
    d
}

async fn have_docker() -> bool {
    tokio::process::Command::new("docker")
        .arg("info")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

async fn have_openssl() -> bool {
    tokio::process::Command::new("openssl")
        .arg("version")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Managed PostgreSQL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn postgres_provision_is_least_privilege_and_redacted() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let admin_url = database_url().expect("db url");
    let provisioner = PostgresProvisioner::new(&admin_url).unwrap();
    let runtime = ProviderRuntime::with_secrets(pool.clone(), vec![SECRET.to_string()]);
    let mut op = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Provision,
    );
    let (_app, _resource) = save_scope(&pool, &mut op).await;

    let outcome = runtime
        .execute_operation(&provisioner, &op, None, Utc::now())
        .await
        .unwrap();
    let detail = match outcome {
        ProviderExecution::Ready { detail } => detail,
        other => panic!("managed provision must be ready, got {other:?}"),
    };
    assert!(detail.contains("least-privilege"), "{detail}");
    assert!(!detail.contains("password"), "{detail}");

    // The credential lives in process memory only.
    let credential = provisioner
        .credential_for(&op.resource_id)
        .expect("credential cached in memory");
    assert_eq!(credential.password.len(), 32);

    // Least privilege, proven with a real connection as the role.
    let role_url = format!(
        "postgres://{}:{}@localhost:5433/{}",
        credential.username, credential.password, credential.database
    );
    let role_pool = PgPool::connect(&role_url).await.expect("connect as role");
    sqlx::query("CREATE TABLE probe_widgets (id serial PRIMARY KEY)")
        .execute(&role_pool)
        .await
        .expect("role writes its own database");
    sqlx::query("INSERT INTO probe_widgets DEFAULT VALUES")
        .execute(&role_pool)
        .await
        .expect("role inserts");
    assert!(
        sqlx::query("CREATE DATABASE probe_must_fail")
            .execute(&role_pool)
            .await
            .is_err(),
        "least-privilege role must not create databases"
    );
    assert!(
        sqlx::query("CREATE ROLE probe_must_fail")
            .execute(&role_pool)
            .await
            .is_err(),
        "least-privilege role must not create roles"
    );
    role_pool.close().await;
    // Other databases grant CONNECT to PUBLIC by default, so the connection
    // itself succeeds; least privilege means no writes and no reads there.
    let foreign_url = format!(
        "postgres://{}:{}@localhost:5433/labrys_real_provider_test",
        credential.username, credential.password
    );
    let foreign_pool = PgPool::connect(&foreign_url)
        .await
        .expect("connect to a foreign database (PUBLIC connect is default)");
    assert!(
        sqlx::query("CREATE TABLE probe_must_fail (id serial PRIMARY KEY)")
            .execute(&foreign_pool)
            .await
            .is_err(),
        "role must not write to other databases"
    );
    assert!(
        sqlx::query("SELECT * FROM provider_operations")
            .fetch_all(&foreign_pool)
            .await
            .is_err(),
        "role must not read other databases"
    );
    foreign_pool.close().await;

    // Readiness through the runtime observes the catalogs, no stored secret.
    let mut check = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::ReadinessCheck,
    );
    check.resource_id = op.resource_id;
    let outcome = runtime
        .execute_operation(&provisioner, &check, None, Utc::now())
        .await
        .unwrap();
    assert!(
        matches!(outcome, ProviderExecution::Ready { .. }),
        "readiness must observe the provisioned role+db, got {outcome:?}"
    );

    // No credential value in any persisted row.
    let rows = sqlx::query("SELECT failure FROM provider_operations")
        .fetch_all(&pool)
        .await
        .unwrap();
    for row in &rows {
        let failure: String = row.try_get("failure").unwrap();
        assert!(
            !failure.contains(&credential.password),
            "operation row leaked"
        );
        assert!(!failure.contains(SECRET), "operation row leaked");
    }
    let events = sqlx::query("SELECT detail_after FROM events")
        .fetch_all(&pool)
        .await;
    if let Ok(events) = events {
        for row in &events {
            let detail: Option<String> = row.try_get("detail_after").unwrap_or(None);
            if let Some(detail) = detail {
                assert!(!detail.contains(&credential.password), "event leaked");
            }
        }
    }

    // Teardown with approval removes role and database.
    let approval = ExplicitApproval::granted(
        "tester",
        format!("delete managed postgres {}", op.resource_id),
    );
    let mut delete = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Delete,
    );
    delete.resource_id = op.resource_id;
    let outcome = runtime
        .execute_operation(&provisioner, &delete, Some(&approval), Utc::now())
        .await
        .unwrap();
    assert!(
        matches!(outcome, ProviderExecution::Ready { .. }),
        "approved delete must remove everything, got {outcome:?}"
    );
    assert!(provisioner.credential_for(&op.resource_id).is_none());
    let maintenance = PgPool::connect(&admin_url).await.unwrap();
    let db_gone: bool =
        sqlx::query("SELECT NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&credential.database)
            .fetch_one(&maintenance)
            .await
            .unwrap()
            .try_get::<bool, _>(0)
            .unwrap();
    assert!(db_gone, "database must be dropped");
    maintenance.close().await;
}

#[tokio::test]
async fn postgres_delete_without_approval_performs_zero_calls() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let admin_url = database_url().expect("db url");
    let provisioner = PostgresProvisioner::new(&admin_url).unwrap();
    let runtime = ProviderRuntime::new(pool.clone());
    let mut op = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Provision,
    );
    let (_app, _resource) = save_scope(&pool, &mut op).await;
    runtime
        .execute_operation(&provisioner, &op, None, Utc::now())
        .await
        .unwrap();

    let mut delete = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Delete,
    );
    delete.resource_id = op.resource_id;
    // Denial happens before any adapter call: the credential stays cached.
    let err = runtime
        .execute_operation(&provisioner, &delete, None, Utc::now())
        .await
        .expect_err("delete without approval must be denied");
    assert!(err.to_string().contains("approval"), "{err}");
    assert!(provisioner.credential_for(&delete.resource_id).is_some());
    // Denial persisted with recovery; the database still exists.
    let stored = runtime
        .get_operation_by_key(&delete.idempotency_key)
        .await
        .unwrap();
    assert_eq!(stored.phase, ResourcePhase::Failed);
    assert!(!stored.failure.is_empty());
    let mut check = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::ReadinessCheck,
    );
    check.resource_id = op.resource_id;
    assert!(
        matches!(
            runtime
                .execute_operation(&provisioner, &check, None, Utc::now())
                .await
                .unwrap(),
            ProviderExecution::Ready { .. }
        ),
        "denied delete must leave the database intact"
    );

    // Cleanup with approval.
    let approval = ExplicitApproval::granted(
        "tester",
        format!("delete managed postgres {}", op.resource_id),
    );
    runtime
        .execute_operation(&provisioner, &delete, Some(&approval), Utc::now())
        .await
        .unwrap();
}

#[tokio::test]
async fn postgres_unsupported_and_adopt_paths_are_explicit() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let admin_url = database_url().expect("db url");
    let provisioner = PostgresProvisioner::new(&admin_url).unwrap();
    let runtime = ProviderRuntime::new(pool.clone());

    let mut replace = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Replace,
    );
    let (_app, _resource) = save_scope(&pool, &mut replace).await;
    let approval = ExplicitApproval::granted(
        "tester",
        format!("replace managed postgres {}", replace.resource_id),
    );
    let outcome = runtime
        .execute_operation(&provisioner, &replace, Some(&approval), Utc::now())
        .await
        .unwrap();
    match outcome {
        ProviderExecution::Failed { reason, recovery } => {
            assert!(reason.contains("does not support"), "{reason}");
            assert!(!recovery.is_empty());
        }
        other => panic!("replace must fail explicitly, got {other:?}"),
    }

    let mut adopt_missing = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Adopt,
    );
    // Same scope as the approval above; nothing was provisioned, so adoption
    // must fail explicitly.
    adopt_missing.resource_id = replace.resource_id;
    let outcome = runtime
        .execute_operation(&provisioner, &adopt_missing, Some(&approval), Utc::now())
        .await
        .unwrap();
    assert!(
        matches!(outcome, ProviderExecution::Failed { .. }),
        "adopting a missing database must fail, got {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// Filesystem storage buckets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn storage_buckets_provision_with_scoped_keys_and_revoke() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    for (kind, capability, key) in [
        (
            ProviderKind::ObjectStorage,
            "storage.object",
            "objectstore.fs",
        ),
        (ProviderKind::FileStorage, "storage.file", "filestore.fs"),
    ] {
        let root = scratch(key);
        let backend = labrys_control_plane::FsBucketBackend::new(root.clone()).unwrap();
        let adapter: Arc<dyn ProviderAdapter> = match kind {
            ProviderKind::ObjectStorage => {
                Arc::new(ObjectStorageProvisioner::with_backend(backend.clone()))
            }
            _ => Arc::new(FileStorageProvisioner::with_backend(backend.clone())),
        };
        let runtime = ProviderRuntime::with_secrets(pool.clone(), vec![SECRET.to_string()]);
        let mut op = provision_op(kind, capability, key, ProviderAction::Provision);
        let (_app, _resource) = save_scope(&pool, &mut op).await;
        let outcome = runtime
            .execute_operation(&*adapter, &op, None, Utc::now())
            .await
            .unwrap();
        let detail = match outcome {
            ProviderExecution::Ready { detail } => detail,
            other => panic!("{key} provision must be ready, got {other:?}"),
        };
        assert!(detail.contains("scoped key"), "{detail}");

        let prefix = if kind == ProviderKind::ObjectStorage {
            "obj"
        } else {
            "file"
        };
        let bucket =
            labrys_control_plane::FsBucketBackend::bucket_name_for(prefix, &op.resource_id)
                .unwrap();
        assert!(root.join(&bucket).exists(), "real bucket directory");
        let scoped = backend.key_for(&bucket).expect("key in memory");
        assert_eq!(scoped.secret.len(), 64);

        // Persisted rows carry the key id and scope, never the secret.
        let stored = runtime
            .get_operation_by_key(&op.idempotency_key)
            .await
            .unwrap();
        assert_eq!(stored.phase, ResourcePhase::Ready);
        let rows = sqlx::query("SELECT failure FROM provider_operations WHERE id = $1")
            .bind(&stored.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let failure: String = rows.try_get("failure").unwrap();
        assert!(!failure.contains(&scoped.secret));
        let events = labrys_control_plane::PgEventStore::new(pool.clone());
        for event in events.for_trace(&op.trace_id).await.unwrap() {
            assert!(
                !format!("{event:?}").contains(&scoped.secret),
                "event leaked the scoped secret"
            );
        }

        // Approved delete revokes the key and removes the directory.
        let approval =
            ExplicitApproval::granted("tester", format!("delete bucket {}", op.resource_id));
        let mut delete = provision_op(kind, capability, key, ProviderAction::Delete);
        delete.resource_id = op.resource_id;
        let outcome = runtime
            .execute_operation(&*adapter, &delete, Some(&approval), Utc::now())
            .await
            .unwrap();
        assert!(
            matches!(outcome, ProviderExecution::Ready { .. }),
            "approved bucket delete must succeed, got {outcome:?}"
        );
        assert!(backend.key_for(&bucket).is_none(), "key revoked");
        assert!(!root.join(&bucket).exists(), "directory removed");
        tokio::fs::remove_dir_all(&root).await.ok();
    }
}

#[tokio::test]
async fn storage_readiness_after_cache_loss_reports_recovery() {
    let root = scratch("cache-loss");
    let first = labrys_control_plane::FsBucketBackend::new(root.clone()).unwrap();
    let adapter = ObjectStorageProvisioner::with_backend(first);
    let op = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::Provision,
    );
    adapter.execute(&op).await.unwrap();
    // A fresh backend over the same root simulates a daemon restart: the
    // bucket is present but the key registry is empty.
    let second = labrys_control_plane::FsBucketBackend::new(root.clone()).unwrap();
    let restarted = ObjectStorageProvisioner::with_backend(second);
    let mut check = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::ReadinessCheck,
    );
    check.resource_id = op.resource_id;
    match restarted.execute(&check).await.unwrap() {
        ProviderExecution::Failed { reason, recovery } => {
            assert!(reason.contains("no live key"), "{reason}");
            assert!(recovery.contains("re-run provision"), "{recovery}");
        }
        other => panic!("restarted readiness must fail with recovery, got {other:?}"),
    }
    tokio::fs::remove_dir_all(&root).await.ok();
}

// ---------------------------------------------------------------------------
// OCI registry delivery
// ---------------------------------------------------------------------------

struct RegistryHarness {
    container: String,
    endpoint: String,
}

impl RegistryHarness {
    async fn start() -> Option<Self> {
        if !have_docker().await {
            eprintln!("registry harness: BLOCKED — no docker daemon; start Docker, then re-run");
            return None;
        }
        // Pull first so a missing image (offline registry mirror) skips
        // instead of failing the suite.
        let pull = tokio::process::Command::new("docker")
            .args(["pull", "registry:2"])
            .output()
            .await
            .ok()?;
        if !pull.status.success() {
            eprintln!("registry harness: BLOCKED — cannot pull registry:2; ensure registry access, then re-run");
            return None;
        }
        let run = tokio::process::Command::new("docker")
            .args(["run", "-d", "--rm", "-p", "127.0.0.1::5000", "registry:2"])
            .output()
            .await
            .ok()?;
        if !run.status.success() {
            return None;
        }
        let container = String::from_utf8_lossy(&run.stdout).trim().to_string();
        for _ in 0..30 {
            let port = tokio::process::Command::new("docker")
                .args(["port", &container, "5000"])
                .output()
                .await
                .ok()?;
            let mapping = String::from_utf8_lossy(&port.stdout).trim().to_string();
            if let Some(addr) = mapping.split_whitespace().last() {
                let endpoint = format!("http://{addr}");
                // Wait for the registry to answer.
                for _ in 0..30 {
                    if reqwest::get(format!("{endpoint}/v2/")).await.is_ok() {
                        return Some(Self {
                            container,
                            endpoint,
                        });
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let _ = tokio::process::Command::new("docker")
            .args(["rm", "-f", &container])
            .output()
            .await;
        None
    }

    async fn stop(&self) {
        let _ = tokio::process::Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output()
            .await;
    }
}

fn test_artifact(reference: &str, manifest: &[u8], revision: &str) -> RegistryArtifact {
    RegistryArtifact::new(
        reference,
        format!("sha256:{}", labrys_control_plane::sha256_hex(manifest)),
        revision,
        true,
    )
}

#[tokio::test]
async fn registry_push_round_trip_verifies_digest() {
    let Some(harness) = RegistryHarness::start().await else {
        return;
    };
    let outcome = registry_round_trip_inner(&harness).await;
    harness.stop().await;
    outcome.expect("registry round trip");
}

async fn registry_round_trip_inner(harness: &RegistryHarness) -> Result<(), String> {
    let Some((_g, pool)) = setup().await else {
        return Err("no database; set LABRYS_DATABASE_URL".to_string());
    };
    let runtime = ProviderRuntime::new(pool.clone());
    let client =
        OciRegistryClient::new(&harness.endpoint, None, None).map_err(|err| err.to_string())?;
    let config = b"{}";
    let config_digest = format!("sha256:{}", labrys_control_plane::sha256_hex(config));
    let manifest = oci_manifest(&config_digest, config.len());
    let app_id = ApplicationId::new().to_string();

    // Verified push round-trips the digest through a real registry.
    let artifact = test_artifact("labrys/e2e", &manifest, "rev-e2e-001");
    let record = runtime
        .push_content(&client, &app_id, &artifact, &manifest, config, Utc::now())
        .await
        .map_err(|err| format!("verified push failed: {err}"))?;
    assert_eq!(record.status, "verified");
    assert_eq!(record.reported_digest, artifact.digest);
    assert!(record.reported_digest.starts_with("sha256:"));

    // A tampered expected digest is refused with recovery and persisted as a
    // mismatch; nothing promotes.
    let tampered = RegistryArtifact::new(
        "labrys/e2e",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "rev-e2e-001",
        true,
    );
    let err = runtime
        .push_content(&client, &app_id, &tampered, &manifest, config, Utc::now())
        .await
        .expect_err("digest mismatch must be refused");
    assert!(err.to_string().contains("digest mismatch"), "{err}");
    let row = sqlx::query(
        "SELECT status FROM registry_deliveries WHERE recorded_digest = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&tampered.digest)
    .fetch_one(&pool)
    .await
    .map_err(|err| format!("mismatch record missing: {err}"))?;
    let status: String = row.try_get("status").map_err(|err| err.to_string())?;
    assert_eq!(status, "mismatch");
    Ok(())
}

#[tokio::test]
async fn registry_refuses_unverified_and_contentless_push() {
    let client = OciRegistryClient::new("http://127.0.0.1:9", None, None).unwrap();
    let artifact = RegistryArtifact::new("app", "sha256:abc", "rev1", false);
    let err = client
        .push_content(&artifact, b"manifest", b"{}")
        .await
        .expect_err("unverified artifact must be refused before any I/O");
    assert!(
        err.to_string().contains("verified production build"),
        "{err}"
    );
    let verified = RegistryArtifact::new("app", "sha256:abc", "rev1", true);
    let err = client
        .push(&verified)
        .await
        .expect_err("contentless push must refuse explicitly");
    assert!(err.to_string().contains("content bytes"), "{err}");
}

// ---------------------------------------------------------------------------
// DNS/TLS domain delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dns_localhost_propagates_unproven_stays_pending() {
    assert_eq!(ResolvingDns.ensure("localhost").await, DnsState::Propagated);
    // An empty DNS label is rejected by the resolver stub before any query,
    // so this stays pending in every environment (even behind wildcard DNS,
    // which answers every well-formed query that reaches it).
    assert!(matches!(
        ResolvingDns.ensure("unresolvable..labrys-test").await,
        DnsState::Pending
    ));
}

#[tokio::test]
async fn tls_issuance_persists_fingerprint_not_keys() {
    if !have_openssl().await {
        eprintln!("tls test: BLOCKED — no openssl CLI; install openssl, then re-run");
        return;
    }
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let tls_dir = scratch("tls");
    let delivery = RealDomainDelivery::new(OpensslTlsIssuer::from_path(tls_dir.clone()).unwrap());
    let runtime = ProviderRuntime::with_secrets(pool.clone(), vec![SECRET.to_string()]);
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "localhost", true);
    let approval = ExplicitApproval::granted("tester", "route localhost");
    let mut state = DomainDelivery::new(domain.id);
    let record = runtime
        .converge_domain(
            &delivery,
            &mut state,
            &domain,
            &deployment,
            &approval,
            Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(record.tls, "issued");
    assert!(
        record.traffic_attached,
        "localhost resolves and the cert is issued"
    );
    assert!(
        record.detail.contains("Fingerprint"),
        "fingerprint evidence: {}",
        record.detail
    );
    assert!(
        !record.detail.contains("PRIVATE"),
        "key material in evidence"
    );

    // Key file exists with owner-only permissions; cert verifies with openssl.
    let key_content = tokio::fs::read(delivery.tls().key_path("localhost"))
        .await
        .expect("key file on disk");
    assert!(String::from_utf8_lossy(&key_content).contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = tokio::fs::metadata(delivery.tls().key_path("localhost"))
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "key file is owner-only");
    }
    let stored: String = sqlx::query("SELECT detail FROM domain_deliveries WHERE id = $1")
        .bind(&record.id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("detail")
        .unwrap();
    assert!(
        !stored.contains("PRIVATE KEY"),
        "persisted detail holds no key"
    );
    assert!(
        stored.contains("Fingerprint"),
        "persisted fingerprint evidence"
    );
    tokio::fs::remove_dir_all(&tls_dir).await.ok();
}

#[tokio::test]
async fn cert_failure_attaches_no_traffic_real_adapter() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let tls_dir = scratch("tls-broken");
    let delivery = RealDomainDelivery::new(
        OpensslTlsIssuer::new("/nonexistent-labrys-openssl".into(), tls_dir.clone()).unwrap(),
    );
    let runtime = ProviderRuntime::new(pool.clone());
    let deployment = promoted_deployment();
    let domain = Domain::new(deployment.environment_id, "localhost", true);
    let approval = ExplicitApproval::granted("tester", "route localhost");
    let mut state = DomainDelivery::new(domain.id);
    // Issuance reports the environment blocker; convergence persists the
    // failure and attaches no traffic.
    let issue = delivery.issue_certificate("localhost").await;
    assert!(issue.is_err(), "missing openssl must fail issuance");
    let err = runtime
        .converge_domain(
            &delivery,
            &mut state,
            &domain,
            &deployment,
            &approval,
            Utc::now(),
        )
        .await
        .expect_err("cert failure must fail convergence");
    assert!(err.to_string().contains("certificate"), "{err}");
    assert!(!state.traffic_attached);
    assert!(matches!(
        state.tls,
        labrys_core::CertificateState::Failed { .. }
    ));
    // The failure row persists (saved before the refused attach) with traffic
    // detached; the query is exact because each test truncates first.
    let row = sqlx::query(
        "SELECT tls, traffic_attached FROM domain_deliveries WHERE hostname = 'localhost'",
    )
    .fetch_one(&pool)
    .await
    .expect("failed delivery row persisted");
    let tls: String = row.try_get("tls").unwrap();
    let attached: bool = row.try_get("traffic_attached").unwrap();
    assert_eq!(tls, "failed");
    assert!(!attached);
    tokio::fs::remove_dir_all(&tls_dir).await.ok();
}

#[tokio::test]
async fn daemon_build_registers_real_storage_from_config() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let base = database_url().expect("db url");
    let storage_root = scratch("daemon-storage");
    let tls_dir = scratch("daemon-tls");
    let config = Config {
        database_url: base,
        worker_id: "real-provider-daemon".to_string(),
        lease_seconds: 60,
        poll_interval_ms: 10,
        drain_timeout_ms: 500,
        max_concurrency: 1,
        run_migrations: false,
        runtime_mode: RuntimeMode::Auto,
        docker_bin: std::path::PathBuf::from("docker"),
        workspace_root: scratch("daemon-workspaces"),
        preview_ttl_secs: 3_600,
        provider_postgres_url: None,
        provider_storage_root: storage_root.clone(),
        provider_registry_endpoint: None,
        provider_registry_username: None,
        provider_registry_password: None,
        provider_tls_dir: tls_dir.clone(),
    };
    let dispatcher = labrys_control_plane::ExecutableDispatcher::build(pool.clone(), &config)
        .unwrap()
        .with_availability(labrys_control_plane::RuntimeAvailability::Available {
            docker_version: "test docker".to_string(),
        });
    assert!(
        dispatcher.registry_client().is_none(),
        "no endpoint, no client"
    );
    let _ = dispatcher.domain_delivery();

    // A storage job routed by provider key provisions for real: the target
    // carries the adapter key, so dispatch reaches the filesystem backend.
    // The target is not a resource id, so the adapter mints a fresh resource
    // scope and the outcome carries it back.
    let job = labrys_core::Job {
        id: labrys_core::JobId::new(),
        target: "objectstore.fs-bucket-probe".to_string(),
        action: labrys_core::JobAction::Provision,
        idempotency_key: labrys_core::IdempotencyKey::new(
            "objectstore.fs-bucket-probe",
            labrys_core::JobAction::Provision,
            1,
        ),
        status: labrys_core::JobStatus::Queued,
        attempts: 0,
        policy: labrys_core::RetryPolicy::default(),
        next_attempt_at: Utc::now(),
        last_error: None,
        enqueued_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let outcome = dispatcher.dispatch(&job).await.unwrap();
    match outcome {
        labrys_control_plane::DispatchOutcome::Ready(_) => {}
        other => panic!("routed storage provision must execute ready, got {other:?}"),
    }
    // Exactly one real bucket directory appeared under the approved root.
    let mut buckets = Vec::new();
    let mut entries = tokio::fs::read_dir(&storage_root).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        buckets.push(entry.file_name().to_string_lossy().to_string());
    }
    assert_eq!(buckets.len(), 1, "one real bucket provisioned: {buckets:?}");
    assert!(buckets[0].starts_with("obj-"), "{buckets:?}");
    // The dispatch stage event carries the job trace.
    let events = labrys_control_plane::PgEventStore::new(pool.clone());
    assert!(
        !events
            .for_trace(&format!("job:{}", job.id))
            .await
            .unwrap()
            .is_empty(),
        "identity-attributed dispatch stage persisted"
    );
    tokio::fs::remove_dir_all(&storage_root).await.ok();
    tokio::fs::remove_dir_all(&tls_dir).await.ok();
}

#[tokio::test]
async fn concurrent_operations_across_adapters_stay_isolated() {
    let Some((_g, pool)) = setup().await else {
        return;
    };
    let admin_url = database_url().expect("db url");
    let provisioner = PostgresProvisioner::new(&admin_url).unwrap();
    let storage_root = scratch("concurrent-storage");
    let storage = ObjectStorageProvisioner::new(storage_root.clone()).unwrap();
    let runtime = ProviderRuntime::new(pool.clone());

    // One postgres + two storage provisions run concurrently on unique scopes.
    let mut pg_op = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Provision,
    );
    let (_app, _resource) = save_scope(&pool, &mut pg_op).await;
    let obj_a = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::Provision,
    );
    let obj_b = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::Provision,
    );
    let (pg, a, b) = tokio::join!(
        runtime.execute_operation(&provisioner, &pg_op, None, Utc::now()),
        runtime.execute_operation(&storage, &obj_a, None, Utc::now()),
        runtime.execute_operation(&storage, &obj_b, None, Utc::now()),
    );
    assert!(matches!(pg.unwrap(), ProviderExecution::Ready { .. }));
    assert!(matches!(a.unwrap(), ProviderExecution::Ready { .. }));
    assert!(matches!(b.unwrap(), ProviderExecution::Ready { .. }));
    // Distinct buckets, distinct keys: no cross-talk.
    let bucket_a =
        labrys_control_plane::FsBucketBackend::bucket_name_for("obj", &obj_a.resource_id).unwrap();
    let bucket_b =
        labrys_control_plane::FsBucketBackend::bucket_name_for("obj", &obj_b.resource_id).unwrap();
    assert_ne!(bucket_a, bucket_b);
    assert!(storage_root.join(&bucket_a).exists());
    assert!(storage_root.join(&bucket_b).exists());
    let key_a = storage.backend().key_for(&bucket_a).unwrap();
    let key_b = storage.backend().key_for(&bucket_b).unwrap();
    assert_ne!(key_a.key_id, key_b.key_id);
    assert_ne!(key_a.secret, key_b.secret);

    // Cleanup: approved deletes for all three (each approval names its
    // target scope, as the destructive gate requires).
    let pg_approval = ExplicitApproval::granted(
        "tester",
        format!("delete managed postgres {}", pg_op.resource_id),
    );
    let a_approval =
        ExplicitApproval::granted("tester", format!("delete bucket {}", obj_a.resource_id));
    let b_approval =
        ExplicitApproval::granted("tester", format!("delete bucket {}", obj_b.resource_id));
    let mut pg_del = provision_op(
        ProviderKind::Postgres,
        "database.postgres",
        "postgres.managed",
        ProviderAction::Delete,
    );
    pg_del.resource_id = pg_op.resource_id;
    let mut a_del = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::Delete,
    );
    a_del.resource_id = obj_a.resource_id;
    let mut b_del = provision_op(
        ProviderKind::ObjectStorage,
        "storage.object",
        "objectstore.fs",
        ProviderAction::Delete,
    );
    b_del.resource_id = obj_b.resource_id;
    let (pg_d, a_d, b_d) = tokio::join!(
        runtime.execute_operation(&provisioner, &pg_del, Some(&pg_approval), Utc::now()),
        runtime.execute_operation(&storage, &a_del, Some(&a_approval), Utc::now()),
        runtime.execute_operation(&storage, &b_del, Some(&b_approval), Utc::now()),
    );
    assert!(matches!(pg_d.unwrap(), ProviderExecution::Ready { .. }));
    assert!(matches!(a_d.unwrap(), ProviderExecution::Ready { .. }));
    assert!(matches!(b_d.unwrap(), ProviderExecution::Ready { .. }));
    assert!(!storage_root.join(&bucket_a).exists());
    assert!(!storage_root.join(&bucket_b).exists());
    tokio::fs::remove_dir_all(&storage_root).await.ok();
}
