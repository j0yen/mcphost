//! PRD-mcphost-chain-friction-fixes-20261016 AC1 (P0) -- `description` is a
//! universal spec field: a chain published with one is accepted, and
//! `tools/list` shows that text; the universal set and every kind's publish
//! schema `properties` agree on it.

use crate::common;
use common::{TempDataDir, TestServer, five_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn chain_publishes_with_description_and_tools_list_shows_it() {
    let dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(five_kinds_registry(&dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Cdft AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.tool_publish", json!({"name": "first", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("echo step tool publishes");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "rollup",
                "kind": "chain",
                "spec": {"steps": [{"tool": "first", "args": {}}], "description": "nightly cost roll-up"},
            }),
        )
        .await
        .expect("a chain spec carrying description must publish");

    let listed = client.tools_list().await.expect("tools/list");
    let want = format!("{ns}.rollup");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(want))
        .unwrap_or_else(|| panic!("{want} not listed"));
    assert_eq!(tool["description"], json!("nightly cost roll-up"));
}

#[tokio::test]
async fn universal_set_and_every_publish_schema_list_description() {
    assert!(mcphost::kinds::UNIVERSAL_SPEC_FIELDS.contains(&"description"));

    let dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(five_kinds_registry(&dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Cdft AC1 Schema Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let publish = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish is listed");
    let branches = publish["inputSchema"]["allOf"].as_array().expect("allOf branches");
    assert!(!branches.is_empty());
    for branch in branches {
        let props = branch["then"]["properties"]["spec"]["properties"]
            .as_object()
            .expect("branch properties");
        assert!(
            props.contains_key("description"),
            "branch {} lacks description: {:?}",
            branch["if"],
            props.keys().collect::<Vec<_>>()
        );
    }
}
