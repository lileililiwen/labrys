//! `secret-encryption-hardening`: secrets are sealed with a real cipher,
//! rotation is auditable and value-free, and guards fail closed.

use std::sync::Mutex;

use chrono::{TimeZone, Utc};

use labrys_core::inspector::ExplicitApproval;
use labrys_core::{
    ApplicationId, CoreError, InjectionGrant, MasterKey, SecretAction, SecretStore,
    SECRET_ENVELOPE_VERSION,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap()
}

fn app() -> ApplicationId {
    ApplicationId::new()
}

fn prod_grant(app: ApplicationId) -> InjectionGrant {
    InjectionGrant::new(app, "production", true)
}

fn store() -> SecretStore {
    SecretStore::new(MasterKey::new(vec![7u8; 32]).unwrap())
}

fn granted() -> ExplicitApproval {
    ExplicitApproval::granted("bob", "rotate credential")
}

// --- Requirement: secrets are sealed with a real cipher ---

#[test]
fn seal_and_reopen_round_trips_and_tamper_is_a_verification_failure() {
    let mut store = store();
    let app_id = app();
    let grant = prod_grant(app_id);
    let sealed = store
        .put(
            "prod.db.url",
            "postgres://user:s3cr3t@db/prod",
            &grant,
            "alice",
            now(),
        )
        .unwrap();
    assert_eq!(sealed.cipher_version, SECRET_ENVELOPE_VERSION);

    // WHEN a value is sealed and reopened with the active key ...
    assert_eq!(
        store.open_envelope(&sealed).unwrap(),
        "postgres://user:s3cr3t@db/prod"
    );

    // WHEN the envelope is opened under the wrong key ...
    let other = SecretStore::new(MasterKey::new(vec![9u8; 32]).unwrap());
    let err = other.open_envelope(&sealed).unwrap_err();

    // THEN authentication fails as a verification failure carrying recovery,
    // without exposing plaintext.
    assert!(matches!(err, CoreError::Verification(_)));
    let message = err.to_string();
    assert!(message.contains("re-seal"));
    assert!(!message.contains("s3cr3t"));
}

#[test]
fn old_envelope_after_rotation_remains_readable_until_rotated() {
    let mut store = store();
    let app_id = app();
    let grant = prod_grant(app_id);
    store
        .put("db.url", "s3cr3t", &grant, "alice", now())
        .unwrap();
    let before = store.sealed("db.url").unwrap().clone();

    // WHEN the reference is rotated ...
    store.rotate("db.url", &granted(), "alice", now()).unwrap();

    // THEN the pre-rotation envelope dispatches on its version and remains
    // readable under the active key.
    assert_eq!(store.open_envelope(&before).unwrap(), "s3cr3t");
    assert_eq!(store.inject(&grant, "db.url").unwrap(), "s3cr3t");
}

// --- Requirement: rotation is auditable and value-free ---

#[test]
fn production_rotation_without_approval_is_denied_before_any_reseal() {
    let mut store = store();
    let app_id = app();
    let grant = prod_grant(app_id);
    store
        .put("db.url", "s3cr3t", &grant, "alice", now())
        .unwrap();
    let before = store.sealed("db.url").unwrap().clone();

    // WHEN rotation is requested on a production scope without a granted
    // named approval ...
    let denied = ExplicitApproval::denied("alice", "rotate db.url");
    let err = store.rotate("db.url", &denied, "alice", now()).unwrap_err();

    // THEN it is denied before any re-seal and the denial carries recovery.
    assert!(matches!(err, CoreError::ApprovalRequired(_)));
    assert!(err.to_string().contains("retry"));
    assert_eq!(store.sealed("db.url").unwrap(), &before);
    assert!(store
        .audit()
        .iter()
        .all(|a| a.action != SecretAction::Rotate));
}

#[test]
fn key_rotation_reseals_everything_and_kills_the_old_key() {
    let old_key = MasterKey::new(vec![7u8; 32]).unwrap();
    let mut store = SecretStore::new(old_key);
    let app_id = app();
    let grant = prod_grant(app_id);
    store
        .put("db.url", "s3cr3t-one", &grant, "alice", now())
        .unwrap();
    let dev = InjectionGrant::new(app_id, "development", false);
    store
        .put("api.key", "test-api-key-two", &dev, "alice", now())
        .unwrap();
    let stale_db = store.sealed("db.url").unwrap().clone();

    // Production scope without approval: denied with zero re-seals.
    let denied = ExplicitApproval::denied("alice", "rotate keys");
    let err = store
        .rotate_key(MasterKey::generate(), &denied, "alice", now())
        .unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired(_)));
    assert_eq!(store.sealed("db.url").unwrap(), &stale_db);

    // With approval every live reference re-seals under the new key.
    let rotated = store
        .rotate_key(MasterKey::generate(), &granted(), "alice", now())
        .unwrap();
    assert_eq!(rotated, 2);
    assert_eq!(store.inject(&grant, "db.url").unwrap(), "s3cr3t-one");
    assert_eq!(store.inject(&dev, "api.key").unwrap(), "test-api-key-two");

    // The old key no longer opens the rotated envelopes.
    let old_store = SecretStore::new(MasterKey::new(vec![7u8; 32]).unwrap());
    assert!(old_store
        .open_envelope(store.sealed("db.url").unwrap())
        .is_err());

    // The audit lineage carries references and versions only.
    let rotates: Vec<_> = store
        .audit()
        .iter()
        .filter(|a| a.action == SecretAction::Rotate)
        .collect();
    assert_eq!(rotates.len(), 2);
    for audit in rotates {
        let detail = audit.to_event_detail();
        assert!(!detail.contains("s3cr3t-one"));
        assert!(!detail.contains("test-api-key-two"));
        assert_eq!(audit.approved_by.as_deref(), Some("bob"));
    }
}

#[test]
fn non_production_key_rotation_needs_no_approval() {
    let mut store = store();
    let dev = InjectionGrant::new(app(), "development", false);
    store
        .put("api.key", "dev-value", &dev, "alice", now())
        .unwrap();
    let rotated = store
        .rotate_key(
            MasterKey::generate(),
            &ExplicitApproval::denied("", ""),
            "alice",
            now(),
        )
        .unwrap();
    assert_eq!(rotated, 1);
    assert_eq!(store.inject(&dev, "api.key").unwrap(), "dev-value");
}

#[test]
fn rotation_under_concurrency_keeps_values_and_versions_monotonic() {
    let store = Mutex::new(store());
    let app_id = app();
    let grant = prod_grant(app_id);
    {
        let mut guard = store.lock().unwrap();
        guard
            .put("db.url", "s3cr3t", &grant, "alice", now())
            .unwrap();
    }
    std::thread::scope(|s| {
        let store_ref = &store;
        for actor in ["t1", "t2", "t3", "t4"] {
            s.spawn(move || {
                let mut guard = store_ref.lock().unwrap();
                guard.rotate("db.url", &granted(), actor, now()).unwrap();
            });
        }
    });
    let guard = store.lock().unwrap();
    assert_eq!(guard.sealed("db.url").unwrap().version, 5);
    assert_eq!(guard.inject(&grant, "db.url").unwrap(), "s3cr3t");
}

// --- Master key loading and hygiene ---

#[test]
fn master_key_loads_from_process_configuration_and_never_serializes() {
    // Missing variable names only the variable, never a value.
    let err = MasterKey::from_env("LABRYS_TEST_MISSING_KEY_VAR").unwrap_err();
    assert!(err.to_string().contains("LABRYS_TEST_MISSING_KEY_VAR"));

    std::env::set_var("LABRYS_TEST_MASTER_KEY_VAR", "test-key-material");
    let key = MasterKey::from_env("LABRYS_TEST_MASTER_KEY_VAR").unwrap();
    std::env::remove_var("LABRYS_TEST_MASTER_KEY_VAR");
    let mut store = SecretStore::new(key);
    let grant = prod_grant(app());
    store
        .put("db.url", "s3cr3t", &grant, "alice", now())
        .unwrap();
    assert_eq!(store.inject(&grant, "db.url").unwrap(), "s3cr3t");

    // Debug renderings never carry key material or plaintext.
    let debug_key = format!("{:?}", MasterKey::new(vec![7u8; 32]).unwrap());
    assert!(!debug_key.contains('7'.to_string().repeat(4).as_str()));
    let debug_store = format!("{store:?}");
    assert!(!debug_store.contains("s3cr3t"));

    // Generated keys are usable and distinct.
    assert_ne!(MasterKey::generate(), MasterKey::generate());
}

#[test]
fn master_key_loads_from_file_without_trailing_newline() {
    let path = std::env::temp_dir().join("labrys-test-master-key");
    std::fs::write(&path, b"file-key-material\n").unwrap();
    let key = MasterKey::from_file(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut store = SecretStore::new(key);
    let grant = prod_grant(app());
    store
        .put("db.url", "s3cr3t", &grant, "alice", now())
        .unwrap();
    assert_eq!(store.inject(&grant, "db.url").unwrap(), "s3cr3t");

    assert!(MasterKey::from_file(&path).is_err());
}

// --- Fail-closed guards across every payload boundary ---

#[test]
fn guards_keep_secret_values_out_of_every_payload_boundary() {
    let mut store = store();
    let app_id = app();
    let grant = prod_grant(app_id);
    store
        .put("db.url", "s3cr3t-db-value", &grant, "alice", now())
        .unwrap();
    store
        .put("api.key", "test-api-key-9f2e", &grant, "alice", now())
        .unwrap();

    // Event, log, evidence, API-response, and dashboard payloads carrying
    // known plaintext are redacted to references only.
    let payloads = [
        "agent completed with postgres://db s3cr3t-db-value attached",
        "build log line with header test-api-key-9f2e against api",
        "evidence summary: connected via s3cr3t-db-value",
        "{\"database_url\": \"s3cr3t-db-value\"}",
        "<dd>s3cr3t-db-value</dd><span>test-api-key-9f2e</span>",
    ];
    for payload in payloads {
        let guarded = store.guard_text("boundary.payload", payload).unwrap();
        assert!(!guarded.contains("s3cr3t-db-value"), "{guarded}");
        assert!(!guarded.contains("test-api-key-9f2e"), "{guarded}");
        assert!(guarded.contains("[redacted:"), "{guarded}");
    }

    // Clean payloads pass through untouched.
    assert_eq!(
        store
            .guard_text("boundary.payload", "plain log line")
            .unwrap(),
        "plain log line"
    );

    // The full audit trail is value-free.
    let audit_text = store
        .audit()
        .iter()
        .map(|a| a.to_event_detail())
        .collect::<Vec<_>>()
        .join("\n");
    let swept = store.guard_text("audit.trail", &audit_text).unwrap();
    assert_eq!(swept, audit_text);
}
