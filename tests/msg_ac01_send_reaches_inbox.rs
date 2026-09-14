//! PRD-mcphost-agent-inbox
//! AC1 (P0) — Given tenants A and B with B's policy `open`, When A calls
//! `host.msg.send(to=[B.address], body="ping")`, Then the result has one
//! `message_id`, a `thread_id`, `delivered_to=[B.address]`, and B's
//! `host.msg.inbox()` returns that message with `from_address` equal to
//! A's namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac1_send_reaches_the_recipients_inbox() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "ping"}))
        .await
        .expect("A sends to B");
    let result = extract_structured(&raw);
    assert_eq!(result["delivered_to"], json!([ns_b]), "{result:?}");
    assert!(result["message_id"].is_string(), "{result:?}");
    assert!(result["thread_id"].is_string(), "{result:?}");
    assert_eq!(result["refused"], json!([]), "{result:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    let messages = inbox["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{inbox:?}");
    assert_eq!(messages[0]["from_address"], json!(ns_a), "{inbox:?}");
    assert_eq!(messages[0]["body"], json!("ping"), "{inbox:?}");
    assert_eq!(messages[0]["message_id"], result["message_id"], "{inbox:?}");
}
