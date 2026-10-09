//! PRD-mcphost-chain-prev-contract AC2 (P0) -- Given a chain whose step 2
//! maps `"action": "$.prev.action"` after a step-1 tool returning
//! `{"action": "create", ...}`, When published or dry-run, Then `verdict:
//! will_fail` with evidence `{step: 2, path: "$.prev.action", did_you_mean:
//! "$.prev.result.action"}` and no `success: true`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::{Value, json};

fn chain_spec() -> Value {
    json!({
        "steps": [
            {"tool": "step1", "args": {"action": "create", "resource": "bucket"}},
            {"tool": "step2", "args": {"action": "$.prev.action"}},
        ]
    })
}

fn assert_action_evidence(structured: &Value) {
    assert_eq!(structured["verdict"], json!("will_fail"), "{structured}");
    assert_ne!(structured["success"], json!(true), "{structured}");
    let evidence = structured["evidence"].as_array().expect("evidence array");
    let entry = evidence
        .iter()
        .find(|e| e["path"] == json!("$.prev.action"))
        .unwrap_or_else(|| panic!("evidence must name $.prev.action: {structured}"));
    assert_eq!(entry["step"], json!(2), "{structured}");
    assert_eq!(entry["did_you_mean"], json!("$.prev.result.action"), "{structured}");
}

async fn setup(label: &str) -> (TestServer, McpClient) {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;
    (server, client)
}

#[tokio::test]
async fn dry_run_names_the_missing_result_hop() {
    let (_server, client) = setup("Chainprev AC2 Dry Run Tenant").await;
    publish(&client, "pipeline", "chain", chain_spec()).await;
    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    assert_action_evidence(&common::extract_structured(&result));
}

#[tokio::test]
async fn publish_response_names_the_missing_result_hop() {
    let (_server, client) = setup("Chainprev AC2 Publish Tenant").await;
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pipeline", "kind": "chain", "spec": chain_spec()}),
        )
        .await
        .expect("host.tool_publish must succeed -- the verdict is advisory");
    assert_action_evidence(&common::extract_structured(&result));
}
