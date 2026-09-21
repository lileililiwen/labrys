use labrys_control_plane::config::{Config, RuntimeMode};
use labrys_control_plane::ControlPlaneError;

fn config_from(pairs: &[(&str, &str)]) -> Result<Config, ControlPlaneError> {
    let map: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::from_lookup(|key| map.get(key).cloned())
}

#[test]
fn defaults_apply_when_only_database_url_is_present() {
    let config = config_from(&[("LABRYS_DATABASE_URL", "postgres://u:p@h:5432/db")]).unwrap();
    assert_eq!(config.database_url, "postgres://u:p@h:5432/db");
    assert_eq!(config.lease_seconds, 60);
    assert_eq!(config.poll_interval_ms, 250);
    assert_eq!(config.max_concurrency, 1);
    assert!(config.run_migrations);
    assert!(config.worker_id.starts_with("worker-"));
    config.validate().unwrap();
}

#[test]
fn database_url_falls_back_to_standard_env_key() {
    let config = config_from(&[("DATABASE_URL", "postgresql://h/db")]).unwrap();
    assert_eq!(config.database_url, "postgresql://h/db");
}

#[test]
fn missing_database_url_is_rejected() {
    let err = config_from(&[]).unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn non_postgres_url_is_rejected() {
    let err = config_from(&[("LABRYS_DATABASE_URL", "mysql://h/db")]).unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn unbounded_lease_is_rejected() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_LEASE_SECONDS", "0"),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn oversized_concurrency_is_rejected() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_MAX_CONCURRENCY", "1000"),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn non_numeric_integer_setting_is_rejected() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_POLL_INTERVAL_MS", "soon"),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn migrations_can_be_disabled() {
    let config = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_RUN_MIGRATIONS", "false"),
    ])
    .unwrap();
    assert!(!config.run_migrations);
}

#[test]
fn execution_fields_carry_bounded_defaults() {
    let config = config_from(&[("LABRYS_DATABASE_URL", "postgres://u:p@h:5432/db")]).unwrap();
    assert_eq!(config.runtime_mode, RuntimeMode::Auto);
    assert_eq!(
        config.docker_bin,
        std::path::PathBuf::from("docker"),
        "{:?}",
        config.docker_bin
    );
    assert!(
        config
            .workspace_root
            .to_string_lossy()
            .ends_with("labrys-workspaces"),
        "{:?}",
        config.workspace_root
    );
    assert_eq!(config.preview_ttl_secs, 3_600);
    config.validate().unwrap();
}

#[test]
fn runtime_mode_parses_all_variants_case_insensitively() {
    for (raw, mode) in [
        ("auto", RuntimeMode::Auto),
        ("DOCKER", RuntimeMode::Docker),
        ("Disabled", RuntimeMode::Disabled),
    ] {
        let config = config_from(&[
            ("LABRYS_DATABASE_URL", "postgres://h/db"),
            ("LABRYS_RUNTIME_MODE", raw),
        ])
        .unwrap();
        assert_eq!(config.runtime_mode, mode, "{raw}");
    }
}

#[test]
fn unknown_runtime_mode_is_rejected_before_any_connection() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_RUNTIME_MODE", "kubernetes"),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
    assert!(err.to_string().contains("LABRYS_RUNTIME_MODE"), "{err}");
}

#[test]
fn empty_docker_bin_is_rejected() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_DOCKER_BIN", "   "),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn empty_workspace_root_is_rejected() {
    let err = config_from(&[
        ("LABRYS_DATABASE_URL", "postgres://h/db"),
        ("LABRYS_WORKSPACE_ROOT", "  "),
    ])
    .unwrap_err();
    assert!(matches!(err, ControlPlaneError::Config(_)), "{err:?}");
}

#[test]
fn preview_ttl_is_bounded_to_a_minute_through_a_day() {
    for raw in ["59", "86401"] {
        let err = config_from(&[
            ("LABRYS_DATABASE_URL", "postgres://h/db"),
            ("LABRYS_PREVIEW_TTL_SECONDS", raw),
        ])
        .unwrap_err();
        assert!(
            matches!(err, ControlPlaneError::Config(_)),
            "{raw}: {err:?}"
        );
    }
    for raw in ["60", "3600", "86400"] {
        let config = config_from(&[
            ("LABRYS_DATABASE_URL", "postgres://h/db"),
            ("LABRYS_PREVIEW_TTL_SECONDS", raw),
        ])
        .unwrap();
        assert_eq!(
            config.preview_ttl_secs,
            raw.parse::<u64>().unwrap(),
            "{raw}"
        );
    }
}
