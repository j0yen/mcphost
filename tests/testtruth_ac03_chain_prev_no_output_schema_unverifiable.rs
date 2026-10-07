//! PRD-mcphost-tool-test-truth AC3 (P0) — Given a chain whose step 1 is an
//! `http` tool with no declared `outputs`, When step 2 maps
//! `$.prev.anything`, Then `verdict: unverifiable` with `reason` naming
//! step 1 and `next` pointing at `host_tool_call`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn prev_mapping_into_a_predecessor_with_no_output_schema_is_unverifiable() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // step1 declares no `outputs` at all.
    let step1_spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
    });
    publish(&client, "step1", "http", step1_spec).await;
    publish(&client, "step2", "echo", json!({"schema": {"type": "object"}})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {}},
            {"tool": "step2", "args": {"rows": "$.prev.anything"}},
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["verdict"], json!("unverifiable"), "{structured}");
    assert_eq!(
        structured["reason"],
        json!("no output schema for step 1"),
        "{structured}"
    );
    assert_eq!(
        structured["next"]["tool"],
        json!("host_tool_call"),
        "{structured}"
    );
}
