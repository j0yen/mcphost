//! AC6 (P0) — Given the message fixtures for the 5 most frequent codes
//! (`args_invalid`, `tool_exception`, `upstream_status`, `tool_not_found`,
//! `rate_limited`), When compared before and after this PRD, Then every
//! `message` is byte-identical.
//!
//! The expected strings are the literals the host emitted before this PRD
//! (the `#[error(...)]` text and the kind-supplied `Structured` message);
//! this PRD edits neither, only `data`.

use mcphost::errors::AppError;
use mcphost::kinds::KindError;
use serde_json::json;

fn message(e: AppError) -> String {
    e.into_error_data().message.to_string()
}

#[test]
fn the_five_most_frequent_codes_keep_their_exact_messages() {
    assert_eq!(
        message(AppError::InvalidArgs("name: required".into())),
        "invalid arguments: name: required"
    );
    assert_eq!(
        message(AppError::from(KindError::structured_with(
            "tool_exception",
            "ValueError: boom at tool.py:2",
            json!({"traceback": "tb"}),
        ))),
        "ValueError: boom at tool.py:2"
    );
    assert_eq!(
        message(AppError::from(KindError::structured_with(
            "upstream_status",
            "upstream returned 503",
            json!({"status": 503}),
        ))),
        "upstream returned 503"
    );
    assert_eq!(
        message(AppError::ToolNotFound("t.no_such".into())),
        "tool not found: t.no_such"
    );
    assert_eq!(
        message(AppError::RateLimited),
        "signup rate limit exceeded for this source; try again later"
    );
}

#[test]
fn no_hint_text_leaks_into_any_message() {
    for err in mcphost::errors::all_variants_for_test() {
        let m = message(err);
        assert!(!m.contains("feedback"), "{m}");
    }
}
