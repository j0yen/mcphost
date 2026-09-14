//! PRD-mcphost-agent-inbox
//! AC13 (P1) — Given B acked message m1 but not m2, When B calls
//! `host.msg.inbox(unread_only=true)`, Then only m2 is returned.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac13_unread_only_filters_out_acked_messages() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let m1_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "m1"}))
        .await
        .expect("send m1");
    let m1 = extract_structured(&m1_raw);
    let m1_id = m1["message_id"].as_str().unwrap().to_string();

    let m2_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "m2"}))
        .await
        .expect("send m2");
    let m2 = extract_structured(&m2_raw);
    let m2_id = m2["message_id"].as_str().unwrap().to_string();

    let acked = client_b
        .tools_call("host.msg.ack", json!({"message_ids": [m1_id.clone()]}))
        .await
        .expect("B acks m1");
    assert_eq!(extract_structured(&acked)["acked"], json!(1));

    let unread_raw = client_b
        .tools_call("host.msg.inbox", json!({"unread_only": true}))
        .await
        .expect("unread inbox");
    let unread = extract_structured(&unread_raw);
    let messages = unread["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{unread:?}");
    assert_eq!(messages[0]["message_id"], json!(m2_id), "{unread:?}");
}
