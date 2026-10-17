//! PRD-mcphost-shared-tools-in-grantee-list
//! AC4 (P0) — a tenant that received no shares lists exactly its own + host
//! tools: no `shared_by` entry, and the count does not move when OTHER
//! tenants share public/group tools it is not a grantee of.

use crate::common::{McpClient, TestServer, signup};
use serde_json::json;

async fn listing(c: &McpClient) -> Vec<serde_json::Value> {
    c.tools_list().await.expect("list")["tools"].as_array().unwrap().clone()
}

#[tokio::test]
async fn stgl_ac04() {
    let server = TestServer::start().await;
    let (_, key_a) = signup(&server.base_url, "STGL4 Owner").await;
    let (_, key_d) = signup(&server.base_url, "STGL4 Bystander").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let d = McpClient::with_bearer(&server.base_url, &key_d);

    d.tools_call("host.tool_publish", json!({"name": "mine", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("D publishes its own");
    let before = listing(&d).await;

    for (name, vis) in [("pubtool", "public"), ("grptool", "group")] {
        a.tools_call("host.tool_publish", json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}))
            .await
            .expect("publish");
        if vis == "group" {
            a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group");
            a.tools_call("host.tool_share", json!({"name": name, "visibility": "group", "group": "g"}))
                .await
                .expect("share");
        } else {
            a.tools_call("host.tool_share", json!({"name": name, "visibility": "public"})).await.expect("share");
        }
    }
    let after = listing(&d).await;
    assert_eq!(before.len(), after.len(), "D's list must not grow with shares D did not receive");
    assert!(after.iter().all(|t| t["_meta"].get("shared_by").is_none()));
}
