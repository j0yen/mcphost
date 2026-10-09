//! PRD-mcphost-args-invalid-message-from-data
//! AC4 — Given a 200-char string sent where an integer is expected, When
//! rendered, Then `got` is 40 chars plus `…` and the message contains that
//! excerpt.

use mcphost::kinds::describe_args_error;
use serde_json::json;

#[test]
fn long_string_got_is_forty_chars_plus_ellipsis() {
    let schema = json!({"type": "object", "properties": {"qty": {"type": "integer"}}});
    let validator = jsonschema::validator_for(&schema).unwrap();
    let args = json!({"qty": "x".repeat(200)});
    let e = validator.validate(&args).expect_err("string for integer");
    let ae = describe_args_error(&e);
    let got = ae.data()["got"].as_str().unwrap().to_string();
    assert_eq!(got.chars().count(), 41, "got: {got}");
    assert!(got.ends_with('…'));
    assert!(got.starts_with("\"xxx"));
    assert!(ae.message().contains(&got), "message: {}", ae.message());
    assert!(!ae.message().contains(&"x".repeat(41)));
}
