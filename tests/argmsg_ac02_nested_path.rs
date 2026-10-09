//! PRD-mcphost-args-invalid-message-from-data
//! AC2 — Given a nested schema `order.items[0].qty`, When the mismatch is at
//! `/order/items/0/qty`, Then the message starts `args.order.items.0.qty:`
//! and `data.argument` is `order`, `data.field` is `qty`.

use mcphost::kinds::describe_args_error;
use serde_json::json;

#[test]
fn nested_mismatch_path_is_dotted_and_argument_field_split() {
    let schema = json!({
        "type": "object",
        "properties": {"order": {"type": "object", "properties": {
            "items": {"type": "array", "items": {"type": "object",
                "properties": {"qty": {"type": "integer"}}}}
        }}}
    });
    let validator = jsonschema::validator_for(&schema).unwrap();
    let args = json!({"order": {"items": [{"qty": "x"}]}});
    let e = validator.validate(&args).expect_err("qty is a string");
    let ae = describe_args_error(&e);
    assert!(
        ae.message().starts_with("args.order.items.0.qty:"),
        "message: {}",
        ae.message()
    );
    let data = ae.data();
    assert_eq!(data["instance_path"], json!("/order/items/0/qty"));
    assert_eq!(data["argument"], json!("order"));
    assert_eq!(data["field"], json!("qty"));
}
