//! PRD-mcphost-kind-ask-routing
//! AC2 (P0) -- Given `host.quickstart {kind: message}`, When called, Then the
//! response has `verdict: recipe`, names `host.msg.send`/`host.msg.inbox`,
//! and includes one example call.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_kind_message_returns_the_messaging_recipe() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute Ask AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "message"}))
        .await
        .expect("an outcome word is a recipe, not an error");
    let body = extract_structured(&result);

    assert_eq!(body["verdict"], json!("recipe"));
    assert_eq!(body["outcome"], json!("message"));
    let verbs = body["verbs"].as_array().expect("verbs array");
    for v in ["host.msg.send", "host.msg.inbox"] {
        assert!(verbs.iter().any(|x| x == v), "verbs missing {v}: {verbs:?}");
    }
    assert_eq!(body["example"]["tool"], json!("host.msg.send"));
    assert!(body["example"]["args"].is_object(), "example: {}", body["example"]);
}
