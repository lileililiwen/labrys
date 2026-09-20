use labrys_core::{
    Application, ApplicationRepository, InMemoryApplicationRepository, Manifest, Origin,
};

fn generated_origin() -> Origin {
    Origin::Generated {
        prompt_summary: Some("team wiki".to_string()),
    }
}

fn imported_git_origin() -> Origin {
    Origin::ImportedGit {
        repository_url: "https://example.com/acme/wiki.git".to_string(),
        branch: "main".to_string(),
        commit_sha: Some("abc123".to_string()),
    }
}

#[test]
fn generated_and_imported_share_application_identity() {
    let generated = Application::new("wiki", generated_origin(), "agent:test");
    let imported = Application::new("wiki", imported_git_origin(), "agent:test");

    // Same application-level shape: status, environments, collections.
    assert_eq!(generated.status, imported.status);
    assert_eq!(
        generated.environments.len(),
        imported.environments.len(),
        "both origins expose development and production environments"
    );
    assert_eq!(generated.collections, imported.collections);
    assert_eq!(
        generated.desired_state.version,
        imported.desired_state.version
    );

    // Only origin metadata differs.
    assert_ne!(generated.origin, imported.origin);
    assert_eq!(generated.origin.kind(), "generated");
    assert_eq!(imported.origin.kind(), "imported_git");
}

#[test]
fn all_six_origins_deserialize_strictly() {
    let parent = Application::new("parent", generated_origin(), "agent:test");
    let origins = vec![
        generated_origin(),
        imported_git_origin(),
        Origin::ImportedLocal {
            path: "/srv/app".to_string(),
        },
        Origin::Template {
            template_id: "next-starter".to_string(),
            template_version: Some("1.0.0".to_string()),
        },
        Origin::Fork {
            parent_application_id: parent.id,
        },
        Origin::External {
            external_ref: "ext-123".to_string(),
        },
    ];
    for origin in origins {
        let app = Application::new("app", origin.clone(), "agent:test");
        let json = app.to_json().expect("serialize");
        let parsed = Application::from_json(&json).expect("deserialize");
        assert_eq!(parsed.origin, origin);
    }
}

#[test]
fn preview_binding_change_does_not_mutate_production() {
    let mut app = Application::new("shop", generated_origin(), "agent:test");
    app.add_preview_environment("pr-42");

    {
        let preview = app.environment_by_name_mut("pr-42").expect("preview env");
        preview
            .refs
            .capabilities
            .insert("postgres".to_string(), "res-preview-1".to_string());
    }

    let production = app
        .environment_by_name("production")
        .expect("production env");
    assert!(
        !production.refs.capabilities.contains_key("postgres"),
        "production binding must remain unchanged"
    );
    let preview = app.environment_by_name("pr-42").expect("preview env");
    assert_eq!(
        preview
            .refs
            .capabilities
            .get("postgres")
            .map(String::as_str),
        Some("res-preview-1")
    );
}

#[test]
fn repository_roundtrips_generated_and_imported_origins() {
    let repo = InMemoryApplicationRepository::new();
    let generated = Application::new("wiki", generated_origin(), "agent:test");
    let imported = Application::new("legacy", imported_git_origin(), "agent:test");

    repo.save(generated.clone()).expect("save generated");
    repo.save(imported.clone()).expect("save imported");

    assert_eq!(repo.get(&generated.id).expect("get"), generated);
    assert_eq!(repo.get(&imported.id).expect("get"), imported);
    assert_eq!(repo.list().len(), 2);

    repo.delete(&generated.id).expect("delete");
    assert_eq!(repo.list().len(), 1);
    assert!(repo.get(&generated.id).is_err());
}

#[test]
fn missing_manifest_uses_database_state_then_exports() {
    // WHEN an imported project lacks labrys.yaml, the database record is truth.
    let app: Application = Application::import_manifest_yaml(
        None,
        "legacy-shop",
        Origin::ImportedLocal {
            path: "/srv/legacy-shop".to_string(),
        },
        "agent:test",
    )
    .expect("import without manifest");
    assert_eq!(app.name, "legacy-shop");

    let repo = InMemoryApplicationRepository::new();
    repo.save(app.clone()).expect("save");
    assert_eq!(repo.get(&app.id).expect("get"), app);

    // AND it can later export a portable manifest that re-imports cleanly.
    let yaml = app.export_manifest_yaml().expect("export manifest");
    assert!(yaml.contains("manifest_version: 1"));
    let manifest = Manifest::from_yaml(&yaml).expect("parse exported manifest");
    let reimported = Manifest::import_application(
        Some(&manifest),
        "fallback",
        generated_origin(),
        "agent:test",
    );
    assert_eq!(reimported.name, app.name);
    assert_eq!(reimported.origin, app.origin);
    assert_eq!(reimported.desired_state, app.desired_state);
}

#[test]
fn manifest_roundtrip_preserves_desired_state_version() {
    let mut app = Application::new("wiki", generated_origin(), "agent:test");
    let next = app
        .desired_state
        .with_config(serde_json::json!({"replicas": 2}));
    app.update_desired_state(next, "agent:test");

    let yaml = app.export_manifest_yaml().expect("export");
    let manifest = Manifest::from_yaml(&yaml).expect("parse");
    let reimported = Manifest::import_application(
        Some(&manifest),
        "fallback",
        generated_origin(),
        "agent:test",
    );
    assert_eq!(reimported.desired_state, app.desired_state);
    assert_eq!(reimported.desired_state.version, 2);
}

#[test]
fn strict_schema_rejects_unknown_fields() {
    let app = Application::new("wiki", generated_origin(), "agent:test");
    let mut value: serde_json::Value =
        serde_json::from_str(&app.to_json().expect("serialize")).expect("json value");
    value["unknown_future_field"] = serde_json::json!("nope");
    let json = serde_json::to_string(&value).expect("reserialize");
    assert!(
        Application::from_json(&json).is_err(),
        "unknown fields must be rejected for strict schemas"
    );

    let yaml = app.export_manifest_yaml().expect("export");
    let with_unknown = format!("{yaml}unknown_future_field: true\n");
    assert!(
        Manifest::from_yaml(&with_unknown).is_err(),
        "manifest must reject unknown fields"
    );
}

#[test]
fn migration_compat_rejects_unsupported_manifest_version() {
    let yaml = r#"
manifest_version: 999
name: future-app
origin:
  type: generated
"#;
    let err = Manifest::from_yaml(yaml).expect_err("future version must fail");
    assert!(
        err.to_string().contains("999"),
        "error must name the offending version, got: {err}"
    );
}

#[test]
fn desired_and_observed_state_stay_separate() {
    let mut app = Application::new("wiki", generated_origin(), "agent:test");
    let desired_version = app.desired_state.version;
    assert!(
        !app.observed_state.is_stale(&app.desired_state),
        "fresh observation matches desired version"
    );

    // Recording an observation never mutates desired state.
    let observed = labrys_core::ObservedState {
        desired_version,
        status: labrys_core::ObservedStatus::Healthy,
        last_observed_at: chrono::Utc::now(),
        detail: Some("all checks passing".to_string()),
    };
    app.record_observation(observed);
    assert_eq!(app.desired_state.version, desired_version);

    // Advancing desired state makes the previous observation stale.
    let next = app
        .desired_state
        .with_config(serde_json::json!({"replicas": 3}));
    app.update_desired_state(next, "agent:test");
    assert!(app.observed_state.is_stale(&app.desired_state));
}

#[test]
fn stable_identifiers_survive_json_roundtrip() {
    let app = Application::new("wiki", generated_origin(), "agent:test");
    let id_string = app.id.to_string();
    let parsed: labrys_core::ApplicationId = id_string.parse().expect("parse id");
    assert_eq!(parsed, app.id);

    let json = app.to_json().expect("serialize");
    let restored = Application::from_json(&json).expect("deserialize");
    assert_eq!(restored.id, app.id);
    assert!(restored.created_at == app.created_at);
    assert_eq!(restored.audit.created_by, "agent:test");
}
