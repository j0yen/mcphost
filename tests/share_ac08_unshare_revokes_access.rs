//! AC8 — Given `host.tool_unshare(name="geo")`, When B calls, Then
//! `tool_not_found`.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn unshare_revokes_a_previously_public_tool() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish geo");
    client_a
        .tools_call("host.tool_share", json!({"name": "geo", "visibility": "public"}))
        .await
        .expect("share geo");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.geo");

    client_b
        .tools_call(&qualified, json!({}))
        .await
        .expect("B can call while shared");

    let unshared = client_a
        .tools_call("host.tool_unshare", json!({"name": "geo"}))
        .await
        .expect("unshare ok");
    assert_eq!(
        common::extract_structured(&unshared)["visibility"],
        "private"
    );

    let err = client_b
        .tools_call(&qualified, json!({}))
        .await
        .expect_err("B must be refused after unshare");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
}
