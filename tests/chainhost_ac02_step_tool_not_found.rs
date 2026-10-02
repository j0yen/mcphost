//! PRD-mcphost-chain-host-steps AC2 (P0) — Given a chain spec whose step
//! names a bare tool that does not exist in the tenant, When
//! `host.spec_test` or `host.tool_publish` runs, Then the error class is
//! `step_tool_not_found`, `data.step` is the index, `data.name` the
//! string, and nothing is published.

use crate::common;
use common::{TestServer, chain_kind_registry, signup};
use serde_json::json;

fn unresolvable_chain_spec() -> serde_json::Value {
    json!({
        "steps": [
            {"tool": "fetch_rowz", "args": {}}
        ]
    })
}

#[tokio::test]
async fn tool_publish_refuses_with_step_tool_not_found_and_publishes_nothing() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chain Unresolved Step Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pipeline", "kind": "chain", "spec": unresolvable_chain_spec()}),
        )
        .await
        .expect_err("a chain naming a nonexistent bare tool must be refused");

    assert_eq!(err.error_code.as_deref(), Some("step_tool_not_found"));
    assert_eq!(err.data["step"], json!(1));
    assert_eq!(err.data["name"], json!("fetch_rowz"));

    // Nothing is published.
    let list = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("host.tool_list must succeed");
    let structured = common::extract_structured(&list);
    let names: Vec<&str> = structured["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"pipeline"), "nothing must be published: {names:?}");
}

#[tokio::test]
async fn spec_test_refuses_with_step_tool_not_found() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Chain Unresolved Step Spec Test Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "chain", "spec": unresolvable_chain_spec(), "invocations": [{}]}),
        )
        .await
        .expect_err("host.spec_test must refuse the same unresolvable step");

    assert_eq!(err.error_code.as_deref(), Some("step_tool_not_found"));
    assert_eq!(err.data["step"], json!(1));
    assert_eq!(err.data["name"], json!("fetch_rowz"));
}
