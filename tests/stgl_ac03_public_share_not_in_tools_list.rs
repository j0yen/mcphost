//! PRD-mcphost-shared-tools-in-grantee-list
//! AC3 (P0) — a `public` share is in NEITHER tenant's `tools/list`, and
//! `host.tool.list_shared` returns it for both.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn stgl_ac03() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL3 Owner").await;
    let (_, key_b) = signup(&server.base_url, "STGL3 B").await;
    let (_, key_c) = signup(&server.base_url, "STGL3 C").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    a.tools_call(
        "host.tool_publish",
        json!({"name": "shy", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");
    a.tools_call("host.tool_share", json!({"name": "shy", "visibility": "public"}))
        .await
        .expect("share");

    let qualified = format!("{ns_a}.shy");
    for key in [&key_b, &key_c] {
        let client = McpClient::with_bearer(&server.base_url, key);
        let listed = client.tools_list().await.expect("list");
        assert!(listed["tools"].as_array().unwrap().iter().all(|t| t["name"] != json!(qualified)));
        let shared = extract_structured(
            &client.tools_call("host.tool.list_shared", json!({})).await.expect("list_shared"),
        );
        let tools = shared["tools"].as_array().expect("tools");
        let entry = tools.iter().find(|t| t["name"] == json!(qualified)).expect("public share listed");
        assert_eq!(entry["_meta"]["shared_visibility"], json!("public"));
    }
}
