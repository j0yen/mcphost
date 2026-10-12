//! AC5 (P0) — Given an `internal` error, When returned, Then the hint's
//! `text` placeholder includes `request_id=<id>` and the id equals
//! `message`'s id.

use mcphost::errors::AppError;

#[test]
fn internal_hint_text_carries_the_messages_request_id() {
    let data = AppError::Internal("pkcs8 decode: secret detail".into()).into_error_data();
    let id = data
        .message
        .strip_prefix("internal error; request_id=")
        .expect("message shape")
        .to_string();
    assert!(!id.is_empty());
    let d = data.data.expect("data");
    let hint = d["feedback_hint"].as_str().expect("hint");
    assert!(hint.contains("kind:\"bug\""), "{hint}");
    assert!(hint.contains(&format!("text:\"<what happened; request_id={id}>\"")), "{hint}");
    assert_eq!(d["request_id"].as_str(), Some(id.as_str()));
    assert!(!hint.contains("secret detail"), "{hint}");
}

#[test]
fn a_non_internal_hint_has_no_request_id() {
    let d = AppError::Forbidden.into_error_data().data.unwrap();
    assert!(!d["feedback_hint"].as_str().unwrap().contains("request_id"));
}
