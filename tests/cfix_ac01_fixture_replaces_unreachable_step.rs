//! PRD-mcphost-chain-dry-run AC1 (P0) -- Given a 3-step chain whose step 1
//! is an http tool at an unreachable host and whose steps 2-3 are echo tools
//! reading `$.prev.result.items`, When `host.tool_test` runs with
//! `fixtures: {"1": {"items": [1, 2]}}`, Then the verdict is `pass`,
//! `steps[0].source` is `fixture`, `steps[1]` and `steps[2]` carry
//! `resolved_args` and `result`, and the http step made no connection attempt.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_fixture_stands_in_for_an_unreachable_first_step() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let fetch = json!({"method": "GET", "url": "http://127.0.0.1:1/x", "args_schema": {"type": "object"}});
    publish(&client, "fetcher", "http", fetch).await;
    publish(&client, "sink_a", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink_b", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({
        "steps": [
            {"tool": "fetcher", "args": {}},
            {"tool": "sink_a", "args": {"rows": "$.prev.result.items"}},
            {"tool": "sink_b", "args": {"rows": "$.prev.result.rows"}},
        ]
    });
    publish(&client, "pipeline", "chain", spec).await;

    // Without the fixture the unreachable step 1 leaves the chain unverifiable.
    let bare = common::extract_structured(
        &client
            .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}}))
            .await
            .expect("host.tool_test"),
    );
    assert_eq!(bare["verdict"], json!("unverifiable"), "{bare}");

    let structured = common::extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                json!({"name": "pipeline", "args": {}, "fixtures": {"1": {"items": [1, 2]}}}),
            )
            .await
            .expect("host.tool_test with fixtures must succeed"),
    );
    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
    let steps = structured["steps"].as_array().expect("steps");
    assert_eq!(steps[0]["source"], json!("fixture"), "{structured}");
    assert_eq!(steps[0]["result"], json!({"items": [1, 2]}), "{structured}");
    assert_eq!(steps[1]["resolved_args"]["rows"], json!([1, 2]), "{structured}");
    assert_eq!(steps[1]["source"], json!("dry_run"), "{structured}");
    assert_eq!(steps[1]["result"]["rows"], json!([1, 2]), "{structured}");
    // Step 3 maps a field step 2's echo does carry (`rows`).
    assert!(steps[2].get("resolved_args").is_some() && steps[2].get("result").is_some(), "{structured}");
}
