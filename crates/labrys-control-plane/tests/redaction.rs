use labrys_control_plane::redact::{ensure_no_secret, redact, redact_guard};
use labrys_control_plane::ControlPlaneError;

const SECRET: &str = "hunter2-supersecret-token";

#[test]
fn redact_strips_known_secret_values() {
    let input = format!("connection failed for {SECRET} on host db-1");
    let out = redact(&input, &[SECRET]);
    assert_eq!(out, "connection failed for [redacted] on host db-1");
    assert!(!out.contains(SECRET));
}

#[test]
fn redact_handles_multiple_secrets() {
    let a = "alpha-secret";
    let b = "beta-secret";
    let out = redact(&format!("{a} and {b} leaked"), &[a, b]);
    assert_eq!(out, "[redacted] and [redacted] leaked");
}

#[test]
fn ensure_no_secret_rejects_residual_plaintext() {
    let err =
        ensure_no_secret("log.message", &format!("value is {SECRET}"), &[SECRET]).unwrap_err();
    match err {
        ControlPlaneError::SecretLeak(field) => assert_eq!(field, "log.message"),
        other => panic!("expected SecretLeak, got {other:?}"),
    }
}

#[test]
fn ensure_no_secret_error_never_echoes_the_value() {
    let err = ensure_no_secret("field", SECRET, &[SECRET]).unwrap_err();
    assert!(!err.to_string().contains(SECRET));
}

#[test]
fn redact_guard_redacts_then_passes() {
    let out = redact_guard("event.before", &format!("token={SECRET}"), &[SECRET]).unwrap();
    assert_eq!(out, "token=[redacted]");
}

#[test]
fn empty_secret_list_leaves_text_unchanged() {
    assert_eq!(redact("nothing to strip", &[]), "nothing to strip");
    assert!(ensure_no_secret("f", "nothing to strip", &[]).is_ok());
}
