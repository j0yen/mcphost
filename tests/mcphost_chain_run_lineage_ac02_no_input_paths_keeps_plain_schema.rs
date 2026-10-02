//! PRD-mcphost-chain-run-lineage AC2 (P0) — Given a chain with no
//! `$.input.*` paths, When published, Then its `input_schema` is exactly
//! `{"type":"object"}`.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn chain_with_no_input_paths_keeps_the_plain_object_schema() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;

    // Neither step references `$.input.*` -- step2 pulls from `$.prev`
    // only, and step1's own arg is a literal.
    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {"n": 5}},
            {"tool": "step2", "args": {"m": "$.prev.result.n"}},
        ]
    });
    let chain = publish(&client, "no_inputs", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.no_inputs"));

    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(chain))
        .expect("published chain listed")
        .clone();

    assert_eq!(
        tool["inputSchema"],
        json!({"type": "object"}),
        "a chain with no $.input.* paths must publish the plain schema exactly: {tool}"
    );
}
