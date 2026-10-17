//! PRD-mcphost-shared-tools-in-grantee-list
//! AC7 (P0) — `host.tool_share` to a group of two returns
//! `listed_for.grantees_notified == 2` and both members' next `tools/list`
//! carries the entry (ttlMs 0, since their listing just changed).

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn stgl_ac07() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL7 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "STGL7 B").await;
    let (ns_c, key_c) = signup(&server.base_url, "STGL7 C").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group");
    for ns in [&ns_b, &ns_c] {
        a.tools_call("host.group.add", json!({"name": "g", "namespace": ns})).await.expect("add");
    }
    a.tools_call("host.tool_publish", json!({"name": "shx", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let shared = extract_structured(
        &a.tools_call("host.tool_share", json!({"name": "shx", "visibility": "group", "group": "g"}))
            .await
            .expect("share"),
    );
    assert_eq!(shared["listed_for"]["grantees_notified"], json!(2), "{shared}");
    assert_eq!(shared["listed_for"]["visibility"], json!("group"));

    let qualified = format!("{ns_a}.shx");
    for key in [&key_b, &key_c] {
        let listed = McpClient::with_bearer(&server.base_url, key).tools_list().await.expect("list");
        assert!(listed["tools"].as_array().unwrap().iter().any(|t| t["name"] == json!(qualified)));
        assert_eq!(listed["ttlMs"], json!(0), "grantee's cache hint must drop after a share: {listed}");
    }
}
