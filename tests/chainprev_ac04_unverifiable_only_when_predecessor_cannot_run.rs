//! PRD-mcphost-chain-prev-contract AC4 (P0) -- Given a predecessor that
//! cannot be dry-run (upstream unreachable in the test), When the chain is
//! dry-run, Then the mapping verdict is `unverifiable` with the reason
//! naming the predecessor, and only then.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn an_unreachable_predecessor_is_unverifiable_and_named() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainprev AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Nothing listens on 127.0.0.1:1, and the spec declares no `outputs`.
    let step1_spec = json!({
        "method": "GET",
        "url": "http://127.0.0.1:1/x",
        "args_schema": {"type": "object"},
    });
    publish(&client, "fetcher", "http", step1_spec).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({
        "steps": [
            {"tool": "fetcher", "args": {}},
            {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
        ]
    });
    publish(&client, "pipeline", "chain", spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["verdict"], json!("unverifiable"), "{structured}");
    let reason = structured["reason"].as_str().expect("reason");
    assert!(reason.contains("step 1") && reason.contains("fetcher"), "{structured}");
}

#[tokio::test]
async fn a_runnable_predecessor_without_outputs_is_never_unverifiable() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chainprev AC4 Runnable Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({
        "steps": [
            {"tool": "src", "args": {"rows": [1, 2]}},
            {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
        ]
    });
    publish(&client, "pipeline", "chain", spec).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("host.tool_test must succeed");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
}
