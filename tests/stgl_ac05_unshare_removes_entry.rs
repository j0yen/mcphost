//! PRD-mcphost-shared-tools-in-grantee-list
//! AC5 (P0) — after A unshares, B's next `tools/list` drops the entry and
//! B's `host.tool_call` returns `shared_tool_not_found`.

use crate::common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn stgl_ac05() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL5 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "STGL5 Member").await;
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
    let qualified = format!("{ns_a}.shx");
    let names = |v: serde_json::Value| -> Vec<String> {
        v["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    };
    assert!(names(b.tools_list().await.unwrap()).contains(&qualified));

    a.tools_call("host.tool_unshare", json!({"name": "shx"})).await.expect("unshare");
    assert!(!names(b.tools_list().await.unwrap()).contains(&qualified));
    let err = b
        .tools_call("host.tool_call", json!({"name": qualified, "args": {}}))
        .await
        .expect_err("call after unshare");
    // `AppError::shared_tool_not_found` is the constructor; its wire code is
    // `tool_not_found` carrying the shared-call hint.
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert!(err.data["hint"].as_str().unwrap_or("").contains("shared tools are called as"), "{err:?}");
}
