//! PRD-mcphost-tool-test-truth AC2 (P0) — Given the same chain as AC1 with
//! step 2 mapped to `$.prev.result`, When `host.tool_test` runs, Then
//! `verdict: pass` and `resolved_args` carries no `unresolved_path`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn prev_mapping_naming_a_declared_output_is_pass() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

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
            {"tool": "step2", "args": {"rows": "$.prev.result"}},
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
    let steps = structured["steps"].as_array().expect("steps report");
    assert_eq!(steps.len(), 2);
    assert!(
        steps[1]["resolved_args"]["rows"].get("unresolved_path").is_none(),
        "a pass verdict's own mapping must carry no unresolved_path: {structured}"
    );
}
