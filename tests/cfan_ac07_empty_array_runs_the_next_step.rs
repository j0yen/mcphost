//! PRD-mcphost-chain-fanout-and-join AC7 (P1) -- Given step 1 returns
//! `{"items": []}`, When called, Then step 2's result is `{items: [],
//! failed: [], ok: 0, failed_count: 0}` and step 3 runs.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn an_empty_list_joins_to_zero_and_the_next_step_runs() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    for name in ["source", "worker", "after"] {
        publish(&client, name, "echo", json!({"schema": schema})).await;
    }
    let chain = publish(
        &client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": "$.input.items"}},
            {"tool": "worker", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
            {"tool": "after", "args": {"ok": "$.prev.result.ok"}},
        ]}),
    )
    .await;

    let call = client.tools_call(&chain, json!({"items": []})).await.expect("an empty fan-out succeeds");
    let structured = extract_structured(&call);
    assert_eq!(structured["failed_steps"], json!([]), "{structured}");
    let steps = structured["steps"].as_array().expect("trace");
    assert_eq!(steps.len(), 3, "step 3 ran: {structured}");
    assert_eq!(
        steps[1]["result"],
        json!({"items": [], "failed": [], "ok": 0, "failed_count": 0}),
        "{structured}"
    );
    assert_eq!(steps[2]["status"], json!("done"), "{structured}");
    assert_eq!(steps[2]["result"]["ok"], json!(0), "step 3 read step 2's join: {structured}");
}
