//! PRD-mcphost-args-invalid-message-from-data
//! AC5 — Given `ArgsError` for 20 generated mismatches, When the drift test
//! serializes `data` and renders `message()`, Then every `field`,
//! `expected_type`, `actual_type` and `got` value in `data` appears verbatim
//! in the message.

use mcphost::kinds::describe_args_error;
use serde_json::{Value, json};

fn wrong_value_for(ty: &str, i: usize) -> Value {
    match ty {
        "integer" => json!(format!("s{i}")),
        "number" => json!([i]),
        "string" => json!(i as f64 + 0.5),
        "boolean" => json!({"k": i}),
        "array" => json!("not-an-array-".repeat(i % 5 + 1)),
        _ => json!(null),
    }
}

#[test]
fn every_data_value_appears_verbatim_in_message_for_twenty_mismatches() {
    let types = ["integer", "number", "string", "boolean", "array"];
    for i in 0..20 {
        let ty = types[i % types.len()];
        let field = format!("f{i}");
        let schema = json!({"type": "object", "properties": {
            "outer": {"type": "object", "properties": {field.clone(): {"type": ty}}}}});
        let validator = jsonschema::validator_for(&schema).unwrap();
        let args = json!({"outer": {field.clone(): wrong_value_for(ty, i)}});
        let e = validator.validate(&args).expect_err("generated mismatch");
        let ae = describe_args_error(&e);
        let data = serde_json::to_value(&ae).unwrap();
        let message = ae.message();
        for key in ["field", "expected_type", "actual_type", "got"] {
            let v = data[key].as_str().unwrap_or_else(|| panic!("case {i}: data.{key} missing: {data}"));
            assert!(message.contains(v), "case {i}: {key}={v:?} not in {message:?}");
        }
        assert_eq!(data["field"], json!(field));
    }
}
