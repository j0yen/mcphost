//! PRD-mcphost-form-tool
//! AC2 (P0) — Given `FORM_INPUTS`, When the test enumerates the types the
//! jsonschema validator accepts for a property, Then every type has a row and
//! a table copy with an extra row fails the drift test; Given publish with
//! `ui: {form: true, bogus: 1}`, Then it fails `check_unknown_spec_field`
//! naming `bogus`.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::form_page::{FORM_INPUTS, check_form_inputs_drift};
use serde_json::json;

/// Probes the real validator: every candidate name is offered as a property
/// `type`, and the ones `jsonschema` builds a validator for are accepted. The
/// `enum` keyword is not a `type`, so it is probed as its own schema.
fn validator_accepted_types() -> Vec<String> {
    let candidates = [
        "string", "number", "integer", "boolean", "array", "object", "null", "text", "float", "int", "bool", "date", "any",
    ];
    let mut accepted: Vec<String> = candidates
        .iter()
        .filter(|t| jsonschema::validator_for(&json!({"type": t})).is_ok())
        .map(|t| t.to_string())
        .collect();
    if jsonschema::validator_for(&json!({"enum": ["a", "b"]})).is_ok() {
        accepted.push("enum".to_string());
    }
    accepted
}

#[test]
fn input_map_covers_every_type_the_validator_accepts() {
    let accepted = validator_accepted_types();
    for expected in ["string", "number", "integer", "boolean", "array", "object", "null", "enum"] {
        assert!(accepted.iter().any(|a| a == expected), "validator should accept {expected}: {accepted:?}");
    }
    assert!(!accepted.iter().any(|a| a == "float"), "probe must reject a non-type: {accepted:?}");
    check_form_inputs_drift(FORM_INPUTS, &accepted).expect("FORM_INPUTS matches the validator's accepted types");
}

#[test]
fn table_copy_with_an_extra_row_fails_the_drift_test() {
    let accepted = validator_accepted_types();
    let mut copy: Vec<(&str, Option<&str>, &str)> = FORM_INPUTS.to_vec();
    copy.push(("float", None, "number"));
    let err = check_form_inputs_drift(&copy, &accepted).expect_err("an extra row must fail");
    assert!(err.contains("float"), "drift error names the extra row: {err}");

    let mut missing: Vec<(&str, Option<&str>, &str)> = FORM_INPUTS.to_vec();
    missing.retain(|(t, _, _)| *t != "boolean");
    assert!(check_form_inputs_drift(&missing, &accepted).is_err(), "a missing row must fail");
}

#[tokio::test]
async fn publish_with_unknown_ui_subfield_names_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftool AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object", "properties": {"text": {"type": "string"}}});

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ask", "kind": "echo", "spec": {"schema": schema, "ui": {"form": true, "bogus": 1}}}),
        )
        .await
        .expect_err("an unknown ui sub-field must fail the publish");
    assert_eq!(err.error_code.as_deref(), Some("unknown_spec_field"));
    assert_eq!(err.data.get("field"), Some(&json!("ui.bogus")));
    assert!(err.message.contains("bogus"), "message names bogus: {}", err.message);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ask", "kind": "echo", "spec": {"schema": schema, "ui": {"form": true, "submit_label": "Send"}}}),
        )
        .await
        .expect("a conforming ui publishes");
}
