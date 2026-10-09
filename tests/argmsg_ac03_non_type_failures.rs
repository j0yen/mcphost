//! PRD-mcphost-args-invalid-message-from-data
//! AC3 — Given a `pattern` or `enum` failure, When rendered, Then the
//! message is `args.<path>: <jsonschema text>; see host.tool_test` and
//! `data` has `field` and `got` but no `expected_type`.

use mcphost::kinds::describe_args_error;
use serde_json::{Value, json};

fn render(schema: Value, args: Value) -> (String, Value, String) {
    let validator = jsonschema::validator_for(&schema).unwrap();
    let e = validator.validate(&args).expect_err("must be invalid");
    let ae = describe_args_error(&e);
    (ae.message(), ae.data(), e.to_string())
}

#[test]
fn pattern_failure_uses_jsonschema_text_and_see_docs() {
    let schema = json!({"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]+$"}}});
    let (message, data, raw) = render(schema, json!({"code": "abc"}));
    assert_eq!(message, format!("args.code: {raw}; see host.tool_test"));
    assert_eq!(data["field"], json!("code"));
    assert_eq!(data["got"], json!("\"abc\""));
    assert!(data.get("expected_type").is_none(), "data: {data}");
    assert!(data.get("actual_type").is_none(), "data: {data}");
}

#[test]
fn enum_failure_uses_jsonschema_text_and_see_docs() {
    let schema = json!({"type": "object", "properties": {"mode": {"enum": ["a", "b"]}}});
    let (message, data, raw) = render(schema, json!({"mode": "z"}));
    assert_eq!(message, format!("args.mode: {raw}; see host.tool_test"));
    assert_eq!(data["field"], json!("mode"));
    assert_eq!(data["got"], json!("\"z\""));
    assert!(data.get("expected_type").is_none(), "data: {data}");
}
