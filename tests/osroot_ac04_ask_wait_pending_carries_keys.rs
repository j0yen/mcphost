//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-tool-call-output-root-opaque
//! AC4 (P0) -- Given an ask that is never answered, When `host.ask.wait` times
//! out, Then the result carries `answered_unix: null` and `choice: null`, and
//! the pending results of `host.ask.wait`, `host.ask.get` and `host.ask.list`
//! (list item) each validate against the advertised `outputSchema`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn output_schema(client: &McpClient, tool: &str) -> Value {
    let listing = client.tools_list().await.expect("tools/list");
    listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} not listed"))["outputSchema"]
        .clone()
}

fn assert_valid(schema: &Value, instance: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).expect("outputSchema compiles");
    let errors: Vec<String> = validator.iter_errors(instance).map(|e| format!("{}: {e}", e.instance_path)).collect();
    assert!(errors.is_empty(), "{what} does not validate: {errors:?}\n{instance}");
}

#[tokio::test]
async fn osroot_ac04_unanswered_ask_wait_carries_null_keys_and_all_three_verbs_validate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Osroot AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let created = extract_structured(
        &client
            .tools_call("host.ask.create", json!({"question": "Ship it?", "options": ["yes", "no"]}))
            .await
            .expect("ask.create"),
    );
    let ask_id = created["ask_id"].as_str().expect("ask_id").to_string();

    let waited = extract_structured(
        &client.tools_call("host.ask.wait", json!({"ask_id": ask_id, "timeout_s": 1})).await.expect("ask.wait"),
    );
    assert_eq!(waited["status"], json!("pending"), "{waited}");
    assert_eq!(waited.get("answered_unix"), Some(&Value::Null), "pending wait must carry answered_unix: null: {waited}");
    assert_eq!(waited.get("choice"), Some(&Value::Null), "pending wait must carry choice: null: {waited}");
    assert_valid(&output_schema(&client, "host.ask.wait").await, &waited, "host.ask.wait pending");

    let got = extract_structured(&client.tools_call("host.ask.get", json!({"ask_id": ask_id})).await.expect("ask.get"));
    assert_eq!(got["status"], json!("pending"), "{got}");
    assert_valid(&output_schema(&client, "host.ask.get").await, &got, "host.ask.get pending");

    let listed = extract_structured(&client.tools_call("host.ask.list", json!({})).await.expect("ask.list"));
    let item = listed["asks"]
        .as_array()
        .expect("asks")
        .iter()
        .find(|a| a["ask_id"] == json!(ask_id) || a["id"] == json!(ask_id))
        .unwrap_or_else(|| panic!("pending ask in {listed}"));
    assert_eq!(item["status"], json!("pending"), "{item}");
    assert_valid(&output_schema(&client, "host.ask.list").await, &listed, "host.ask.list with a pending item");

    // The three verbs return one shape for a pending ask.
    for (what, row) in [("wait", &waited), ("get", &got), ("list item", item)] {
        assert_eq!(row.get("answered_unix"), Some(&Value::Null), "{what}: {row}");
        assert_eq!(row.get("choice"), Some(&Value::Null), "{what}: {row}");
    }
}
