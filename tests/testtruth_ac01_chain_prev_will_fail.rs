//! PRD-mcphost-tool-test-truth AC1 (P0) — Given a chain whose step 2 maps
//! `rows: "$.prev.rows"` and whose step 1 tool declares outputs `result`,
//! `count`, When `host.tool_test` runs, Then `verdict: will_fail` with one
//! failure `{step: 2, path: "$.prev.rows", predecessor_keys: ["result",
//! "count"]}` and no step executed.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn prev_mapping_absent_from_predecessor_outputs_is_will_fail() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // step1 declares outputs `result`, `count` -- never actually reached
    // (the dry run resolves this purely from the published spec), so the
    // URL need not be reachable.
    let step1_spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
        "outputs": ["result", "count"],
    });
    publish(&client, "step1", "http", step1_spec).await;
    publish(&client, "step2", "echo", json!({"schema": {"type": "object"}})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {}},
            {"tool": "step2", "args": {"rows": "$.prev.rows"}},
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed -- it never dispatches a step");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["verdict"], json!("will_fail"), "{structured}");
    assert_eq!(
        structured["failures"],
        json!([{
            "step": 2,
            "path": "$.prev.rows",
            "predecessor_keys": ["result", "count"],
        }]),
        "{structured}"
    );
    assert_eq!(
        structured["steps"],
        json!([]),
        "a will_fail verdict must execute no step: {structured}"
    );
}
