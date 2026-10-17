//! PRD-mcphost-publish-visibility-and-tags AC7 (P1) -- Given two concurrent
//! `host.tool_share visibility=public` calls on the same untested tool, When
//! both dry runs pass, Then `tested_version` is stamped once and both calls
//! return the same result without a UNIQUE or busy error.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn pubvis_ac07() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Pubvis AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "racy", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("private publish");

    let share = json!({"name": "racy", "visibility": "public", "description": "racy tool"});
    let other = McpClient::with_bearer(&server.base_url, &key);
    let (a, b) = tokio::join!(
        client.tools_call("host.tool_share", share.clone()),
        other.tools_call("host.tool_share", share.clone()),
    );
    let a = extract_structured(&a.expect("first concurrent share"));
    let b = extract_structured(&b.expect("second concurrent share"));
    assert_eq!(a, b, "both calls return the same result");
    assert_eq!(a["visibility"], json!("public"), "{a}");
    assert_eq!(a["tested_version"], json!(1), "{a}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    let tool = list["tools"].as_array().expect("tools").iter().find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".racy"))).expect("racy");
    assert_eq!((tool["tested_version"].clone(), tool["visibility"].clone()), (json!(1), json!("public")), "{list}");
}
