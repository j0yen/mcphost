//! PRD-mcphost-publish-visibility-and-tags AC5 (P0) -- Given
//! `tags: ["Webhook", "webhook", "a b"]`, When published, Then the response is
//! `args_invalid` naming `a b`; given `["Webhook", "webhook"]`, Then stored
//! tags are `["webhook"]`; given 17 valid tags, Then `args_invalid` names the
//! limit 16.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn publish(tags: serde_json::Value) -> serde_json::Value {
    json!({"name": "tagged", "kind": "echo", "spec": {"schema": {"type": "object"}}, "tags": tags})
}

#[tokio::test]
async fn pubvis_ac05() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Pubvis AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call("host.tool_publish", publish(json!(["Webhook", "webhook", "a b"])))
        .await
        .expect_err("a tag with a space is invalid");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("a b"), "message names the bad tag: {err:?}");

    let ok = extract_structured(
        &client.tools_call("host.tool_publish", publish(json!(["Webhook", "webhook"]))).await.expect("publish"),
    );
    assert_eq!(ok["tags"], json!(["webhook"]), "{ok}");
    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    let stored = list["tools"].as_array().expect("tools").iter().find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".tagged"))).expect("tagged tool");
    assert_eq!(stored["tags"], json!(["webhook"]), "{list}");

    let seventeen: Vec<String> = (0..17).map(|i| format!("tag{i}")).collect();
    let err = client
        .tools_call("host.tool_publish", publish(json!(seventeen)))
        .await
        .expect_err("17 tags is over the limit");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("16"), "message names the limit: {err:?}");
}
