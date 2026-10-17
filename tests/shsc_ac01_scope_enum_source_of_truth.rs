//! PRD-mcphost-scoped-tool-share
//! AC1 (P0) -- Given a group share with `scopes: ["read_source"]`, When a
//! member calls the tool, Then `scope_missing` with `required: "call"` and
//! `granted: ["read_source"]`; and the drift test finds `host.tool_share`'s
//! `scopes.items.enum == ShareScope::ALL`.

use serde_json::{Value, json};

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::sharing::ShareScope;

/// Owner publishes `name` (echo) and shares it to a group holding `member_ns`.
async fn share_to_group(owner: &McpClient, member_ns: &str, name: &str, extra: Value) {
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    owner.tools_call("host.group.create", json!({"name": "crew"})).await.expect("group create");
    owner
        .tools_call("host.group.add", json!({"name": "crew", "namespace": member_ns}))
        .await
        .expect("group add");
    let mut args = json!({"name": name, "visibility": "group", "group": "crew"});
    for (k, v) in extra.as_object().expect("extra is an object") {
        args[k] = v.clone();
    }
    owner.tools_call("host.tool_share", args).await.expect("share");
}

#[tokio::test]
async fn shsc_ac01_read_source_only_share_refuses_call() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Member B").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_a);
    let member = McpClient::with_bearer(&server.base_url, &key_b);

    share_to_group(&owner, &ns_b, "scoped", json!({"scopes": ["read_source"]})).await;

    let err = member
        .tools_call(&format!("{ns_a}.scoped"), json!({}))
        .await
        .expect_err("a read_source-only share must refuse a call");
    assert_eq!(err.error_code.as_deref(), Some("scope_missing"), "{}", err.message);
    assert_eq!(err.data["required"], json!("call"));
    assert_eq!(err.data["granted"], json!(["read_source"]));
}

#[tokio::test]
async fn shsc_ac01_schema_enum_equals_share_scope_all() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Lister").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let listed = client.tools_list().await.expect("tools/list");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| matches!(t["name"].as_str(), Some("host.tool.share") | Some("host.tool_share")))
        .expect("host.tool_share is listed");
    let en = &tool["inputSchema"]["properties"]["scopes"]["items"]["enum"];
    let all: Vec<&str> = ShareScope::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(en, &json!(all), "host.tool_share scopes enum must equal ShareScope::ALL");
}
