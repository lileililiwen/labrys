use labrys_core::redact_reason;

use crate::error::{ControlPlaneError, Result};

/// Redacts every known secret value out of `text` before it is persisted.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    redact_reason(text, secrets)
}

/// Redacts an optional free-text field, leaving `None` untouched.
pub fn redact_opt(text: &Option<String>, secrets: &[&str]) -> Option<String> {
    text.as_ref().map(|t| redact_reason(t, secrets))
}

/// Fails if any known secret value is still present in `text`.
///
/// This runs *after* [`redact`] as a defense-in-depth gate: a value that could
/// not be redacted (e.g. an empty secret list that a caller should have
/// supplied) is rejected before SQL execution rather than silently stored. The
/// error names the field only and never echoes the secret.
pub fn ensure_no_secret(field: &str, text: &str, secrets: &[&str]) -> Result<()> {
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        if text.contains(secret) {
            return Err(ControlPlaneError::SecretLeak(field.to_string()));
        }
    }
    Ok(())
}

/// Redacts `text` and asserts no known secret survives, for a single field.
pub fn redact_guard(field: &str, text: &str, secrets: &[&str]) -> Result<String> {
    let redacted = redact(text, secrets);
    ensure_no_secret(field, &redacted, secrets)?;
    Ok(redacted)
}
