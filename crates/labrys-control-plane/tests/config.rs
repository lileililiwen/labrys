use labrys_control_plane::config::Config;
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
