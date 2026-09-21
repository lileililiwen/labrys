use chrono::{TimeZone, Utc};

use labrys_core::inspector::ExplicitApproval;
use labrys_core::{
    generic_auth_binding, generic_file_storage_binding, generic_object_storage_binding,
    generic_postgres_binding, resolve_binding, ApplicationId, Binding, BindingKind,
    BoundCapability, Capability, CapabilityMode, CoreError, EnvSource, InjectionGrant, MasterKey,
    Provider, ResourceRef, SecretAction, SecretStore, CAPABILITY_CONTRACT_VERSION,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
}

fn app() -> ApplicationId {
    ApplicationId::new()
}

fn grant(env: &str, production: bool) -> InjectionGrant {
    InjectionGrant::new(app(), env, production)
}

fn postgres_provider(key: &str, supports_reversal: bool, modes: Vec<CapabilityMode>) -> Provider {
    Provider::new(
        key,
        "database.postgres",
        CAPABILITY_CONTRACT_VERSION,
        supports_reversal,
        modes,
    )
}

fn all_modes() -> Vec<CapabilityMode> {
    vec![
        CapabilityMode::Managed,
        CapabilityMode::External,
        CapabilityMode::Adopted,
    ]
}

fn framework_binding(key: &str, framework: &str, provider: &str) -> Binding {
    let mut env = labrys_core::capability::EnvContract::new();
    env.insert(
        format!("{framework}_URL"),
        EnvSource::SecretRef(format!("{provider}.database_url")),
    );
    Binding::new(
        key,
        "database.postgres",
        provider,
        1,
        BindingKind::Framework {
            framework: framework.to_string(),
        },
        env,
    )
}

// --- Requirement: Capability, Provider, and Binding are distinct objects ---

#[test]
fn capability_provider_and_binding_are_separate_versioned_objects() {
    let capability =
        Capability::new("database.postgres", 1).with_dependencies(vec!["network.vpc".to_string()]);
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let binding = generic_postgres_binding("postgres.managed");

    assert_eq!(capability.key, provider.capability);
    assert_eq!(binding.capability, capability.key);
    assert_eq!(binding.provider, provider.key);
    assert_eq!(capability.version, 1);
    assert_eq!(provider.version, CAPABILITY_CONTRACT_VERSION);
    assert_eq!(binding.version, 1);

    // Each object serializes on its own; the binding does not embed the
    // provider or the capability definition.
    let json = serde_json::to_string(&binding).unwrap();
    let back: Binding = serde_json::from_str(&json).unwrap();
    assert_eq!(back, binding);
    assert!(!json.contains("postgres.managed\" :"));
}

#[test]
fn postgres_resolves_distinct_bindings_per_framework() {
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let ef = framework_binding("postgres.efcore", "efcore", "postgres.managed");
    let sqlx = framework_binding("postgres.sqlx", "sqlx", "postgres.managed");
    let generic = generic_postgres_binding("postgres.managed");
    let bindings = [ef.clone(), sqlx.clone(), generic.clone()];
    let providers = [provider.clone()];

    // WHEN one application uses EF Core and another uses SQLx ...
    let a = resolve_binding(
        &bindings,
        &providers,
        "database.postgres",
        "postgres.managed",
        Some("efcore"),
    )
    .unwrap();
    let b = resolve_binding(
        &bindings,
        &providers,
        "database.postgres",
        "postgres.managed",
        Some("sqlx"),
    )
    .unwrap();

    // THEN both resolve database.postgres through distinct bindings ...
    assert_eq!(a.key, "postgres.efcore");
    assert_eq!(b.key, "postgres.sqlx");

    // ... against the same provider identity, independent of framework.
    assert_eq!(a.provider, b.provider);
    assert!(provider.supplies("database.postgres"));
    assert!(!provider.supplies("storage.object"));
}

#[test]
fn rust_project_falls_back_to_generic_database_url() {
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let ef = framework_binding("postgres.efcore", "efcore", "postgres.managed");
    let generic = generic_postgres_binding("postgres.managed");
    let bindings = [ef, generic.clone()];
    let providers = [provider];

    // WHEN a Rust project (no deep binding) requests PostgreSQL ...
    let resolved = resolve_binding(
        &bindings,
        &providers,
        "database.postgres",
        "postgres.managed",
        Some("rust"),
    )
    .unwrap();

    // THEN it receives the generic DATABASE_URL binding and is not blocked.
    assert_eq!(resolved, &generic);
    assert!(resolved.is_generic());
    assert!(matches!(
        resolved.env.get("DATABASE_URL"),
        Some(EnvSource::SecretRef(_))
    ));
}

#[test]
fn every_supported_capability_offers_a_generic_binding() {
    let cases: [(labrys_core::capability::Binding, &str, &str); 4] = [
        (
            generic_postgres_binding("p"),
            "database.postgres",
            "DATABASE_URL",
        ),
        (
            generic_object_storage_binding("p"),
            "storage.object",
            "OBJECT_STORAGE_URL",
        ),
        (generic_auth_binding("p"), "auth", "AUTH_URL"),
        (
            generic_file_storage_binding("p"),
            "storage.file",
            "FILE_STORAGE_URL",
        ),
    ];
    for (binding, capability, primary_env) in cases {
        assert!(binding.is_generic());
        assert_eq!(binding.capability, capability);
        assert!(binding.env.contains_key(primary_env));
        assert!(!binding.secret_refs().is_empty());
    }
}

// --- Binding resolution failures ---

#[test]
fn unknown_provider_is_rejected() {
    let bindings = [generic_postgres_binding("postgres.managed")];
    let providers = [postgres_provider("postgres.managed", true, all_modes())];
    let err = resolve_binding(
        &bindings,
        &providers,
        "database.postgres",
        "postgres.unknown",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));
}

#[test]
fn provider_mismatch_is_rejected() {
    let auth = Provider::new("auth0", "auth", 1, true, all_modes());
    let bindings = [generic_postgres_binding("auth0")];
    let providers = [auth];
    // Provider exists but does not supply the requested capability.
    let err =
        resolve_binding(&bindings, &providers, "database.postgres", "auth0", None).unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));
}

#[test]
fn missing_binding_is_rejected() {
    let bindings: [Binding; 0] = [];
    let providers = [postgres_provider("postgres.managed", true, all_modes())];
    let err = resolve_binding(
        &bindings,
        &providers,
        "database.postgres",
        "postgres.managed",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));
}

#[test]
fn binding_conflicts_are_rejected() {
    let providers = [postgres_provider("postgres.managed", true, all_modes())];

    // Two generic bindings for the same capability/provider pair.
    let conflicting_generic = [
        generic_postgres_binding("postgres.managed"),
        generic_postgres_binding("postgres.managed"),
    ];
    let err = resolve_binding(
        &conflicting_generic,
        &providers,
        "database.postgres",
        "postgres.managed",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));

    // Two deep bindings claiming the same framework.
    let duplicated = [
        framework_binding("postgres.efcore.a", "efcore", "postgres.managed"),
        framework_binding("postgres.efcore.b", "efcore", "postgres.managed"),
        generic_postgres_binding("postgres.managed"),
    ];
    let err = resolve_binding(
        &duplicated,
        &providers,
        "database.postgres",
        "postgres.managed",
        Some("efcore"),
    )
    .unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));
}

// --- Requirement: managed / external / adopted modes with auditable transitions ---

#[test]
fn bind_validates_resource_provider_and_mode() {
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let binding = generic_postgres_binding("postgres.managed");
    let matching = ResourceRef::new("database.postgres", "postgres.managed", "prod-db");

    // Resource that does not match the binding pair is rejected.
    let wrong_resource = ResourceRef::new("storage.object", "postgres.managed", "bucket");
    assert!(BoundCapability::bind(
        app(),
        "production",
        &binding,
        &provider,
        CapabilityMode::Managed,
        wrong_resource,
        "alice",
        now()
    )
    .is_err());

    // Provider that does not operate in the requested mode is rejected.
    let managed_only = postgres_provider("postgres.managed", true, vec![CapabilityMode::Managed]);
    assert!(BoundCapability::bind(
        app(),
        "production",
        &binding,
        &managed_only,
        CapabilityMode::Adopted,
        matching.clone(),
        "alice",
        now()
    )
    .is_err());

    // The valid combination binds with an initial audited record.
    let bound = BoundCapability::bind(
        app(),
        "production",
        &binding,
        &provider,
        CapabilityMode::Managed,
        matching,
        "alice",
        now(),
    )
    .unwrap();
    assert_eq!(bound.mode, CapabilityMode::Managed);
    assert_eq!(bound.history.len(), 1);
    assert_eq!(bound.history[0].approved_by, "alice");
}

#[test]
fn existing_database_becomes_adopted_without_touching_the_resource() {
    let provider = postgres_provider("postgres.external", true, all_modes());
    let binding = generic_postgres_binding("postgres.external");
    let resource = ResourceRef::new("database.postgres", "postgres.external", "prod-db");

    let mut bound = BoundCapability::bind(
        app(),
        "production",
        &binding,
        &provider,
        CapabilityMode::External,
        resource,
        "alice",
        now(),
    )
    .unwrap();

    // WHEN a user approves adoption of the external database ...
    let approval = ExplicitApproval::granted("bob", "adopt prod-db");
    bound
        .transition(CapabilityMode::Adopted, &provider, &approval, now())
        .unwrap();

    // THEN the record keeps the source, approval, binding, and resulting mode ...
    assert_eq!(bound.mode, CapabilityMode::Adopted);
    let transition = bound.history.last().unwrap();
    assert_eq!(transition.to, CapabilityMode::Adopted);
    assert_eq!(transition.approved_by, "bob");
    assert_eq!(
        transition.source.as_deref(),
        Some("postgres.external:prod-db")
    );
    assert_eq!(bound.history.len(), 2);
    assert_eq!(bound.binding, "postgres.generic");

    // AND the existing resource handle is untouched — never recreated.
    assert_eq!(bound.resource.external_ref, "prod-db");
    assert!(bound.to_event_detail().contains("Adopted"));
    assert!(!bound.to_event_detail().contains("password"));
}

#[test]
fn mode_transition_requires_named_human_approval() {
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let binding = generic_postgres_binding("postgres.managed");
    let resource = ResourceRef::new("database.postgres", "postgres.managed", "db");
    let mut bound = BoundCapability::bind(
        app(),
        "production",
        &binding,
        &provider,
        CapabilityMode::Managed,
        resource,
        "alice",
        now(),
    )
    .unwrap();

    // Denied approval is rejected.
    let denied = ExplicitApproval::denied("bob", "adopt db");
    let err = bound
        .transition(CapabilityMode::Adopted, &provider, &denied, now())
        .unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired(_)));

    // Granted approval that names no approver is rejected.
    let anonymous = ExplicitApproval {
        approved: true,
        approver: "  ".to_string(),
        proposal_summary: "adopt db".to_string(),
    };
    assert!(bound
        .transition(CapabilityMode::Adopted, &provider, &anonymous, now())
        .is_err());

    // Same-mode transitions are rejected as no-ops.
    let granted = ExplicitApproval::granted("bob", "stay managed");
    assert!(bound
        .transition(CapabilityMode::Managed, &provider, &granted, now())
        .is_err());
    assert_eq!(bound.history.len(), 1);
}

#[test]
fn reversal_out_of_adopted_requires_provider_support() {
    let provider = postgres_provider("postgres.adopted", false, all_modes());
    let binding = generic_postgres_binding("postgres.adopted");
    let resource = ResourceRef::new("database.postgres", "postgres.adopted", "legacy-db");
    let mut bound = BoundCapability::bind(
        app(),
        "production",
        &binding,
        &provider,
        CapabilityMode::Adopted,
        resource,
        "alice",
        now(),
    )
    .unwrap();

    // WHEN the provider cannot hand the resource back ...
    let approval = ExplicitApproval::granted("bob", "release db");
    let err = bound
        .transition(CapabilityMode::External, &provider, &approval, now())
        .unwrap_err();

    // THEN the reversal is refused and the mode is unchanged.
    assert!(matches!(err, CoreError::CapabilityBinding(_)));
    assert_eq!(bound.mode, CapabilityMode::Adopted);

    // A provider that supports reversal can go back to external.
    let reversible = postgres_provider("postgres.adopted", true, all_modes());
    bound
        .transition(CapabilityMode::External, &reversible, &approval, now())
        .unwrap();
    assert_eq!(bound.mode, CapabilityMode::External);
    assert_eq!(bound.history.len(), 2);
}

#[test]
fn bound_capability_round_trips_through_json() {
    let provider = postgres_provider("postgres.managed", true, all_modes());
    let binding = generic_postgres_binding("postgres.managed");
    let resource = ResourceRef::new("database.postgres", "postgres.managed", "db");
    let bound = BoundCapability::bind(
        app(),
        "staging",
        &binding,
        &provider,
        CapabilityMode::Managed,
        resource,
        "alice",
        now(),
    )
    .unwrap();

    let json = bound.to_json().unwrap();
    let back = BoundCapability::from_json(&json).unwrap();
    assert_eq!(back, bound);
    assert_eq!(back.id, bound.id);
    assert_eq!(back.resource.id, bound.resource.id);
}

// --- Requirement: secret values never enter agent context ---

fn store() -> SecretStore {
    SecretStore::new(MasterKey::new(vec![7u8; 32]).unwrap())
}

#[test]
fn secrets_are_sealed_at_rest_and_never_leak_through_debug_or_audit() {
    let mut store = store();
    let g = grant("production", true);
    let sealed = store
        .put(
            "prod.db.url",
            "postgres://user:s3cr3t@db/prod",
            &g,
            "alice",
            now(),
        )
        .unwrap();

    assert_eq!(sealed.version, 1);
    // The Debug rendering of the store and the sealed record shows no plaintext.
    let debug = format!("{store:?}");
    assert!(!debug.contains("s3cr3t"));
    assert!(!format!("{sealed:?}").contains("s3cr3t"));

    // The agent-safe reference carries the id and a version hint, not the value.
    let reference = sealed.reference();
    assert_eq!(reference.ref_id, "prod.db.url");
    assert_eq!(reference.redacted_hint.as_deref(), Some("v1"));

    // Audit detail names only the reference id.
    let audit = store.audit().first().unwrap().to_event_detail();
    assert!(audit.contains("prod.db.url"));
    assert!(!audit.contains("s3cr3t"));
}

#[test]
fn redaction_strips_known_plaintext_from_events_and_logs() {
    let mut store = store();
    let g = grant("production", true);
    store
        .put("api.key", "sk-live-9f2e", &g, "alice", now())
        .unwrap();

    let event = store.redact("agent ran curl with header sk-live-9f2e against api");
    assert!(!event.contains("sk-live-9f2e"));
    assert!(event.contains("[redacted:api.key]"));

    // Unrelated text passes through untouched.
    assert_eq!(store.redact("plain log line"), "plain log line");
}

#[test]
fn injection_requires_an_authorized_grant() {
    let mut store = store();
    let prod = grant("production", true);
    let secret_app = prod.application_id;
    store
        .put("db.url", "s3cr3t-value", &prod, "alice", now())
        .unwrap();

    // Correct grant reveals the value.
    assert_eq!(store.inject(&prod, "db.url").unwrap(), "s3cr3t-value");

    // A grant for another environment is refused.
    let staging = InjectionGrant::new(secret_app, "staging", false);
    let err = store.inject(&staging, "db.url").unwrap_err();
    assert!(matches!(err, CoreError::CapabilityBinding(_)));

    // A grant for another application is refused.
    let other_app = InjectionGrant::new(app(), "production", true);
    assert!(store.inject(&other_app, "db.url").is_err());
}

#[test]
fn missing_secrets_fail_injection_and_binding_injection() {
    let empty = store();
    let g = grant("production", true);

    // Unknown reference id.
    let err = empty.inject(&g, "db.url").unwrap_err();
    assert!(matches!(err, CoreError::SecretNotFound(_)));

    // A binding referencing a secret that was never stored fails wholesale.
    let binding = generic_postgres_binding("postgres.managed");
    let err = empty.inject_binding(&g, &binding).unwrap_err();
    assert!(matches!(err, CoreError::SecretNotFound(_)));

    // Once stored, the whole env contract injects.
    let mut filled = store();
    filled
        .put(
            "postgres.managed.database_url",
            "url-value",
            &g,
            "alice",
            now(),
        )
        .unwrap();
    let injected = filled.inject_binding(&g, &binding).unwrap();
    assert_eq!(injected.get("DATABASE_URL").unwrap(), "url-value");
}

#[test]
fn rotation_bumps_version_and_keeps_the_value_and_scope() {
    let mut store = store();
    let g = grant("production", true);
    store.put("db.url", "s3cr3t", &g, "alice", now()).unwrap();

    // Production rotation without a granted named approval is denied
    // before any re-seal, with recovery.
    let denied = ExplicitApproval::denied("alice", "rotate db.url");
    let err = store.rotate("db.url", &denied, "alice", now()).unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired(_)));
    assert!(err.to_string().contains("retry"));
    assert_eq!(store.sealed("db.url").unwrap().version, 1);

    let approval = ExplicitApproval::granted("bob", "rotate db.url");
    let version = store.rotate("db.url", &approval, "alice", now()).unwrap();
    assert_eq!(version, 2);
    assert_eq!(store.sealed("db.url").unwrap().version, 2);
    assert_eq!(store.inject(&g, "db.url").unwrap(), "s3cr3t");

    let rotate_audit = store
        .audit()
        .iter()
        .find(|a| a.action == SecretAction::Rotate)
        .unwrap();
    assert!(!rotate_audit.to_event_detail().contains("s3cr3t"));
    assert_eq!(rotate_audit.approved_by.as_deref(), Some("bob"));

    // Rotating an unknown reference fails.
    assert!(matches!(
        store.rotate("nope", &approval, "alice", now()).unwrap_err(),
        CoreError::SecretNotFound(_)
    ));
}

#[test]
fn production_secret_replacement_requires_human_approval() {
    let mut store = store();
    let prod = grant("production", true);
    store
        .put("db.url", "old-s3cr3t", &prod, "alice", now())
        .unwrap();

    // WHEN an agent requests replacement of a production secret without a
    // granted, named approval ...
    let denied = ExplicitApproval::denied("alice", "rotate credential");
    let err = store
        .replace("db.url", "new-s3cr3t", &prod, &denied, "agent", now())
        .unwrap_err();
    assert!(matches!(err, CoreError::ApprovalRequired(_)));

    // THEN the stored value is unchanged.
    assert_eq!(store.inject(&prod, "db.url").unwrap(), "old-s3cr3t");

    // With human approval the replacement succeeds and neither old nor new
    // value appears in the audit record or its event detail.
    let approved = ExplicitApproval::granted("bob", "rotate credential");
    let audit = store
        .replace("db.url", "new-s3cr3t", &prod, &approved, "agent", now())
        .unwrap();
    assert_eq!(audit.action, SecretAction::Replace);
    assert_eq!(audit.approved_by.as_deref(), Some("bob"));
    let detail = audit.to_event_detail();
    assert!(!detail.contains("old-s3cr3t"));
    assert!(!detail.contains("new-s3cr3t"));
    assert_eq!(store.inject(&prod, "db.url").unwrap(), "new-s3cr3t");

    // Replacing an unknown secret fails.
    assert!(matches!(
        store
            .replace("nope", "x", &prod, &approved, "agent", now())
            .unwrap_err(),
        CoreError::SecretNotFound(_)
    ));
}

#[test]
fn non_production_replacement_does_not_require_approval() {
    let mut store = store();
    let dev = grant("development", false);
    store.put("db.url", "old", &dev, "alice", now()).unwrap();
    let audit = store
        .replace(
            "db.url",
            "new",
            &dev,
            &ExplicitApproval::denied("", ""),
            "alice",
            now(),
        )
        .unwrap();
    assert_eq!(audit.approved_by, None);
    assert_eq!(store.inject(&dev, "db.url").unwrap(), "new");
}

#[test]
fn delete_removes_sealed_values() {
    let mut store = store();
    let g = grant("production", true);
    store.put("db.url", "s3cr3t", &g, "alice", now()).unwrap();

    store.delete("db.url", "alice", now()).unwrap();
    assert!(!store.contains("db.url"));
    assert!(store.sealed("db.url").is_none());
    assert!(matches!(
        store.inject(&g, "db.url").unwrap_err(),
        CoreError::SecretNotFound(_)
    ));
    assert!(matches!(
        store.delete("db.url", "alice", now()).unwrap_err(),
        CoreError::SecretNotFound(_)
    ));

    // The delete audit record carries no value.
    let delete_audit = store
        .audit()
        .iter()
        .find(|a| a.action == SecretAction::Delete)
        .unwrap();
    assert!(!delete_audit.to_event_detail().contains("s3cr3t"));
}

#[test]
fn empty_master_key_is_rejected() {
    assert!(MasterKey::new(Vec::new()).is_err());
}
