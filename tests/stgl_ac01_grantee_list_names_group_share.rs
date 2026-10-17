//! PRD-mcphost-shared-tools-in-grantee-list
//! AC1 (P0) — Given owner A shares tool `x` to a group containing B, When B
//! calls `tools/list`, Then `<A ns>.x` is present with `_meta.shared_by` and
//! `_meta.shared_visibility == "group"`, and `host.tool_call` of it succeeds.

use crate::common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn stgl_ac01() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL1 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "STGL1 Member").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group");
    a.tools_call("host.group.add", json!({"name": "g", "namespace": ns_b})).await.expect("add");
    a.tools_call(
        "host.tool_publish",
        json!({"name": "shx", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");

    let qualified = format!("{ns_a}.shx");
    let before = b.tools_list().await.expect("list before share");
    assert!(before["tools"].as_array().unwrap().iter().all(|t| t["name"] != json!(qualified)));

    a.tools_call("host.tool_share", json!({"name": "shx", "visibility": "group", "group": "g"}))
        .await
        .expect("share");
    let listed = b.tools_list().await.expect("list after share");
    let entry = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!(qualified))
        .unwrap_or_else(|| panic!("{qualified} missing from B's tools/list"));
    assert_eq!(entry["_meta"]["shared_by"], json!(ns_a));
    assert_eq!(entry["_meta"]["shared_visibility"], json!("group"));
    b.tools_call("host.tool_call", json!({"name": qualified, "args": {}}))
        .await
        .expect("B calls the shared tool");
}
