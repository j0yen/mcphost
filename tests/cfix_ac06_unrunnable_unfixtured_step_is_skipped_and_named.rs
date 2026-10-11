//! PRD-mcphost-chain-dry-run AC6 (P0) -- Given a chain whose step 2 cannot be
//! dry-run and no fixture for it, When `host.tool_test` runs with a fixture
//! for step 1 only, Then `steps[1].source` is `skipped` with a `reason` and
//! the verdict is `unverifiable` naming step 2.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn an_unrunnable_unfixtured_step_is_skipped_and_the_verdict_names_it() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "head", "echo", json!({"schema": {"type": "object"}})).await;
    let down = json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": {"type": "object"}});
    publish(&client, "flaky", "http", down).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [
        {"tool": "head", "args": {"seed": 1}},
        {"tool": "flaky", "args": {"seed": "$.prev.result.items"}},
        {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
    ]});
    publish(&client, "pipeline", "chain", spec).await;

    let structured = common::extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                json!({"name": "pipeline", "args": {}, "fixtures": {"1": {"items": [1]}}}),
            )
            .await
            .expect("host.tool_test with a step-1 fixture"),
    );
    assert_eq!(structured["steps"][0]["source"], json!("fixture"), "{structured}");
    let step2 = &structured["steps"][1];
    assert_eq!(step2["source"], json!("skipped"), "{structured}");
    assert!(step2["reason"].as_str().is_some_and(|r| !r.is_empty()), "{structured}");
    assert_eq!(structured["verdict"], json!("unverifiable"), "{structured}");
    let reason = structured["reason"].as_str().expect("verdict reason");
    assert!(reason.contains("step 2") && reason.contains("flaky"), "{structured}");
    assert!(structured["skipped"].as_u64().unwrap_or(0) >= 1, "{structured}");
}
