//! PRD-mcphost-agent-inbox
//! AC4 (P0) — Given A sends with `dedupe_key="k1"` and the call is
//! repeated three times, When B reads its inbox, Then exactly one message
//! exists and all three send results carry the same `message_id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac4_resend_with_same_dedupe_key_is_idempotent() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let mut message_ids = Vec::new();
    for _ in 0..3 {
        let raw = client_a
            .tools_call(
                "host.msg.send",
                json!({"to": [ns_b.clone()], "body": "ping", "dedupe_key": "k1"}),
            )
            .await
            .expect("resend with same dedupe_key");
        let result = extract_structured(&raw);
        message_ids.push(result["message_id"].as_str().unwrap().to_string());
    }
    assert_eq!(message_ids[0], message_ids[1]);
    assert_eq!(message_ids[1], message_ids[2]);

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    let messages = inbox["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{inbox:?}");
    assert_eq!(messages[0]["message_id"], json!(message_ids[0]), "{inbox:?}");
}
