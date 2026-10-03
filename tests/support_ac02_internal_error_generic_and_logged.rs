//! PRD-mcphost-first-hour-support-surface
//! AC2 (P0) — Given `AppError::Internal("Stripe checkout session request:
//! card_declined …")`, When returned to a client, Then the message is the
//! fixed generic string, the payload has `request_id`, the body contains
//! neither "Stripe" nor "card", and the server log has one error line with
//! that request id and the full text.
//!
//! The logging half (one `tracing::error!` line carrying `request_id` and
//! the full `detail`) is unchanged from the mcphost-polish-p0-20260930
//! hotfix this PRD generalises -- `errors.rs`'s own
//! `internal_error_response_hides_detail_and_carries_request_id` test
//! already covers the zero-arg `into_error_data()` path byte-for-byte; this
//! file exercises the new `into_error_data_at` path (the one every live
//! HTTP call site actually uses) end to end, including the `help_url` this
//! PRD adds on top.
//!
//! `internal_error_logs_one_error_line_with_request_id_and_full_detail`
//! below is the AC's own "server log" clause -- `tests/support/
//! busyaudit.rs`'s `capture_tracing` (same helper `plancat_ac02` uses)
//! installs a scoped subscriber around the call and asserts on what it
//! actually wrote, so deleting the `tracing::error!` call in `errors.rs`
//! fails this test instead of leaving it green.

use crate::busyaudit;

use mcphost::errors::AppError;

#[test]
fn internal_error_hides_stripe_detail_but_carries_request_id_and_help_url() {
    let detail = "Stripe checkout session request: card_declined (card ending 4242)";
    let response = AppError::Internal(detail.to_string())
        .into_error_data_at(Some("https://mcphost.dev"));

    assert!(
        !response.message.to_lowercase().contains("stripe"),
        "client message must not name the upstream: {}",
        response.message
    );
    assert!(
        !response.message.to_lowercase().contains("card"),
        "client message must not carry card detail: {}",
        response.message
    );
    assert!(
        response.message.starts_with("internal error; request_id="),
        "client message must be the fixed generic string: {}",
        response.message
    );

    let data = response.data.expect("error data object");
    assert_eq!(data["error_code"], "internal");
    let request_id = data["request_id"].as_str().expect("request_id");
    assert!(!request_id.is_empty());
    assert!(
        response.message.ends_with(request_id),
        "message's own request_id must be the same one in data: {}",
        response.message
    );
    assert!(
        !data.to_string().to_lowercase().contains("stripe"),
        "the structured error data must not carry the detail either: {data}"
    );

    let help_url = data["help_url"].as_str().expect("internal must have a help_url");
    assert_eq!(help_url, "https://mcphost.dev/help/internal");
}

#[test]
fn internal_error_logs_one_error_line_with_request_id_and_full_detail() {
    let detail = "Stripe checkout session request: card_declined (card ending 4242)";

    let (response, log) = busyaudit::capture_tracing(|| {
        AppError::Internal(detail.to_string()).into_error_data_at(Some("https://mcphost.dev"))
    });

    let data = response.data.expect("error data object");
    let request_id = data["request_id"].as_str().expect("request_id").to_string();

    let error_lines: Vec<&str> = log.lines().filter(|l| l.contains("service error")).collect();
    assert_eq!(
        error_lines.len(),
        1,
        "expected exactly one 'service error' log line, got:\n{log}"
    );
    let line = error_lines[0];
    assert!(
        line.contains(&request_id),
        "the log line must carry the same request_id the response carries ({request_id}): {line}"
    );
    assert!(
        line.contains(detail),
        "the log line must carry the full, un-redacted detail text: {line}"
    );
}
