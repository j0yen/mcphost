//! PRD-mcphost-shared-tools-in-grantee-list
//! AC6 (P1) — a sub-key whose allowlist does not name the shared tool lists
//! no shared entries (`permits` applies as to own entries); one that names
//! it by qualified name does.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn stgl_ac06() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL6 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "STGL6 Member").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group");
    a.tools_call("host.group.add", json!({"name": "g", "namespace": ns_b})).await.expect("add");
    a.tools_call("host.tool_publish", json!({"name": "shx", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    a.tools_call("host.tool_share", json!({"name": "shx", "visibility": "group", "group": "g"}))
        .await
        .expect("share");
    b.tools_call("host.tool_publish", json!({"name": "mine", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("B publishes");
    let qualified = format!("{ns_a}.shx");

    let mint = |tools: Vec<String>, label: &'static str| {
        let b = &b;
        async move {
            let m = extract_structured(
                &b.tools_call("host.key.mint", json!({"tools": tools, "label": label})).await.expect("mint"),
            );
            m["key"].as_str().expect("key").to_string()
        }
    };
    let narrow = McpClient::with_bearer(&server.base_url, &mint(vec!["mine".into()], "narrow").await);
    let listed = narrow.tools_list().await.expect("narrow list");
    assert!(
        listed["tools"].as_array().unwrap().iter().all(|t| t["_meta"].get("shared_by").is_none()),
        "no shared entries for a key that doesn't permit them"
    );

    let wide = McpClient::with_bearer(&server.base_url, &mint(vec![qualified.clone()], "wide").await);
    let listed = wide.tools_list().await.expect("wide list");
    assert!(listed["tools"].as_array().unwrap().iter().any(|t| t["name"] == json!(qualified)));
}
