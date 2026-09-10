//! AC9 — Given `admin.tool_unshare(tenant=A, name="geo")`, When run, Then
//! the tool is private and the owner's `host.tool_list` shows
//! `visibility: private, unshared_by: admin`.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_unshare_forces_private_and_is_visible_to_the_owner() {
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

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tool_unshare", json!({"tenant": ns_a, "name": "geo"}))
        .await
        .expect("admin unshare ok");

    let list = extract_structured(
        &client_a
            .tools_call("host.tool_list", json!({}))
            .await
            .expect("tool_list"),
    );
    let geo = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == format!("{ns_a}.geo"))
        .expect("geo listed");
    assert_eq!(geo["visibility"], "private");
    assert_eq!(geo["unshared_by"], "admin");

    // A non-admin tenant cannot reach admin.tool_unshare at all.
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let err = client_b
        .tools_call("admin.tool_unshare", json!({"tenant": ns_a, "name": "geo"}))
        .await
        .expect_err("tenant must be forbidden from admin.*");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));
}
