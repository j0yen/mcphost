//! PRD-mcphost-first-hour-support-surface
//! AC3 (P0) — Given `disk_floor` with 12345 bytes free, When returned, Then
//! the client message contains no digits and the code is
//! `service_unavailable`; the 5xx-message test passes over every 5xx
//! variant.
//!
//! "Every 5xx variant" is scoped to the codes this PRD's own requirement 1/6
//! actually found leaking (`internal`, `storage`, `registry_rejected`,
//! `service_unavailable`) -- every other `Structured` code that maps to
//! `INTERNAL_ERROR` today (`sandbox_unavailable`, `dependency_advisory`,
//! `plan_required`, `call_timeout`, ...) is operator-authored, intentionally
//! informative copy already pinned verbatim by its own PRD's acceptance
//! criteria (e.g. `tests/limits_ac01_declared_timeout_honored.rs` asserting
//! `call_timeout`'s message names the deadline in digits) -- see
//! `errors.rs`'s `into_error_data_at` doc comment for the full reasoning.

use mcphost::errors::AppError;

/// The fixed-prefix part of a genericized message, with the trailing
/// `request_id=<hex>` (which legitimately contains digits -- it's a hex
/// id, not leaked data) stripped before the digit check.
fn prefix_before_request_id(message: &str) -> &str {
    message.split("request_id=").next().unwrap_or(message)
}

fn assert_generic(code: &str, message: &str, must_not_contain: &[&str]) {
    assert!(
        message.contains("request_id="),
        "{code}: message must carry a request_id: {message}"
    );
    let prefix = prefix_before_request_id(message);
    assert!(
        !prefix.chars().any(|c| c.is_ascii_digit()),
        "{code}: message must contain no digits outside its own request_id: {message}"
    );
    assert!(!prefix.contains(':'), "{code}: message must carry no ':' after the code word: {message}");
    for needle in must_not_contain {
        assert!(
            !message.contains(needle),
            "{code}: message must not leak {needle:?}: {message}"
        );
    }
}

#[test]
fn disk_floor_message_has_no_digits_and_code_is_service_unavailable() {
    let err = AppError::disk_floor(12_345, 500_000_000);
    assert_eq!(err.code(), "service_unavailable");
    let response = err.into_error_data();
    assert_generic("service_unavailable", &response.message, &["12345", "500000000"]);
    // The raw numbers still reach whoever reads `data` (an operator), just
    // never the client-facing `message` -- requirement 1's own distinction.
    let data = response.data.expect("data");
    assert_eq!(data["free_bytes"], 12_345);
    assert_eq!(data["floor_bytes"], 500_000_000);
}

#[test]
fn storage_message_is_generic() {
    let response = AppError::Storage("database disk image is malformed at /data/mcphost.db".to_string())
        .into_error_data();
    assert_eq!(response.data.as_ref().unwrap()["error_code"], "storage");
    assert_generic("storage", &response.message, &["mcphost.db", "malformed"]);
}

#[test]
fn registry_rejected_message_is_generic() {
    let response =
        AppError::RegistryRejected("422 Unprocessable: duplicate server name 'acme-tools'".to_string())
            .into_error_data();
    assert_eq!(response.data.as_ref().unwrap()["error_code"], "registry_rejected");
    assert_generic("registry_rejected", &response.message, &["acme-tools", "Unprocessable"]);
}

#[test]
fn internal_message_is_generic() {
    let response = AppError::Internal("pkcs8 decode failed: invalid tag".to_string()).into_error_data();
    assert_eq!(response.data.as_ref().unwrap()["error_code"], "internal");
    assert_generic("internal", &response.message, &["pkcs8", "invalid tag"]);
}
