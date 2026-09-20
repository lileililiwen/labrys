/// Errors for the foundation model.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("unsupported manifest version {found}, expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("application not found: {0}")]
    NotFound(String),
    #[error("explicit approval required: {0}")]
    ApprovalRequired(String),
    #[error("policy denied action: {0}")]
    PolicyDenied(String),
    #[error("agent session state violation: {0}")]
    SessionState(String),
    #[error("unsupported agent protocol version {found}, expected {expected}")]
    ProtocolVersion { found: u32, expected: u32 },
    #[error("secret value must not enter agent context: {0}")]
    SecretLeak(String),
    #[error("agent prompt timed out: {0}")]
    Timeout(String),
    #[error("change sequence violation: {0}")]
    SequenceViolation(String),
    #[error("unsupported runtime: {0}")]
    UnsupportedRuntime(String),
    #[error("invalid runtime configuration: {0}")]
    InvalidRuntime(String),
    #[error("sandbox limit exceeded ({limit}): {detail}")]
    LimitExceeded { limit: String, detail: String },
    #[error("workspace state violation: {0}")]
    WorkspaceState(String),
    #[error("preview unavailable: {0}")]
    PreviewUnavailable(String),
    #[error("capability binding violation: {0}")]
    CapabilityBinding(String),
    #[error("secret not found: {0}")]
    SecretNotFound(String),
    #[error("reconciliation violation: {0}")]
    Reconciliation(String),
    #[error("deployment violation: {0}")]
    Deployment(String),
    #[error("verification violation: {0}")]
    Verification(String),
    #[error("audit integrity violation: {0}")]
    AuditIntegrity(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, CoreError>;

impl From<serde_json::Error> for CoreError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

impl From<serde_yaml::Error> for CoreError {
    fn from(err: serde_yaml::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}
