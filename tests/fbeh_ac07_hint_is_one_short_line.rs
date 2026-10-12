//! AC7 (P1) — Given any error, When the hint is measured, Then it is
//! <= 200 bytes and contains no newline.

use mcphost::errors::{AppError, all_variants_for_test};
use mcphost::feedback::{FEEDBACK_HINT_MAX_BYTES, feedback_hint};

fn assert_one_short_line(hint: &str) {
    assert!(hint.len() <= FEEDBACK_HINT_MAX_BYTES, "{} bytes: {hint}", hint.len());
    assert!(!hint.contains('\n') && !hint.contains('\r'), "{hint:?}");
}

#[test]
fn every_variants_hint_is_short_and_single_line() {
    for err in all_variants_for_test() {
        let data = err.into_error_data().data.unwrap();
        assert_one_short_line(data["feedback_hint"].as_str().unwrap());
    }
}

#[test]
fn hostile_tool_names_and_ids_cannot_break_the_bound() {
    let long = "a".repeat(5000);
    let nasty = "x\ny\"z\\\r".repeat(50);
    for tool in [Some(long.as_str()), Some(nasty.as_str()), Some(""), None] {
        for id in [None, Some(long.as_str())] {
            for code in ["internal", "tool_not_found", long.as_str()] {
                assert_one_short_line(&feedback_hint(code, tool, id));
            }
        }
    }
    let data = AppError::ToolNotFound(format!("t.{long}\n")).into_error_data().data.unwrap();
    assert_one_short_line(data["feedback_hint"].as_str().unwrap());
}
