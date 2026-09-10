//! AC3 — Given `geo` shared to group `team` containing C but not B, When B
//! calls, Then `tool_not_found`; When C calls, Then success.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn group_shared_tool_is_reachable_only_by_members() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes geo");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let (ns_c, key_c) = signup(&server.base_url, "Tenant C").await;
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);

    client_a
        .tools_call("host.group.create", json!({"name": "team"}))
        .await
        .expect("A creates group team");
    client_a
        .tools_call("host.group.add", json!({"name": "team", "namespace": ns_c}))
        .await
        .expect("A adds C to team");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "geo", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("A shares geo to team");

    let qualified = format!("{ns_a}.geo");

    let err = client_b
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("B is not in team, must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));

    let call = client_c
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("C is in team, must succeed");
    assert_eq!(extract_structured(&call), json!({"msg": "hi"}));
}
