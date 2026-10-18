//! PRD-mcphost-retry-policy-table-generated AC3 (R2) — Given an
//! `upstream_rate_limited` error with `retry_after_s: 60`, When rendered,
//! Then `data.retry` is `{retryable: true, after_ms: 60000, attempts_left: 2,
//! policy: "..."}`. Also the guardrail: the block adds <= 160 bytes.

use mcphost::errors::AppError;
use serde_json::{Value, json};

fn render(code: &'static str, data: Value) -> Value {
    let err = AppError::Structured { code, message: "upstream is rate limiting".to_string(), data };
    err.into_error_data().data.expect("error data")
}

#[test]
fn rpol_ac03() {
    let data = render("upstream_rate_limited", json!({"retry_after_s": 60}));
    let retry = &data["retry"];
    assert_eq!(retry["retryable"], json!(true));
    assert_eq!(retry["after_ms"], json!(60000));
    assert_eq!(retry["attempts_left"], json!(2));
    assert!(retry["policy"].as_str().is_some_and(|p| !p.is_empty()), "policy is a non-empty string: {retry}");

    // Without an upstream wait, the row's own backoff answers (attempt 1: base).
    let bare = render("upstream_rate_limited", Value::Null);
    assert_eq!(bare["retry"]["after_ms"], json!(1000));

    // The signup path spells it `retry_after_secs`; it is honoured too.
    let signup = render("signup_paused", json!({"retry_after_secs": 90}));
    assert_eq!(signup["retry"]["after_ms"], json!(90000));

    // Guardrail: error payload growth <= 160 bytes.
    let size = serde_json::to_string(retry).expect("serialize retry").len();
    assert!(size <= 160, "data.retry is {size} bytes");
}
