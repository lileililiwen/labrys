use thiserror::Error;

/// Errors surfaced by the durable control plane.
///
/// Variants never carry a secret value: `SecretLeak` reports the offending
/// field name only, and free-text diagnostics are redacted before they reach a
/// variant.
#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("mapping error: {0}")]
    Mapping(String),

    #[error("job state violation: {0}")]
    JobState(String),

    #[error("domain error: {0}")]
    Core(#[from] labrys_core::CoreError),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("generation conflict: {0}")]
    Conflict(String),

    #[error("environment isolation violation: {0}")]
    EnvironmentIsolation(String),

    #[error("secret value rejected before persistence in field '{0}'")]
    SecretLeak(String),

    #[error("worker is shutting down: {0}")]
    Shutdown(String),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, ControlPlaneError>;
