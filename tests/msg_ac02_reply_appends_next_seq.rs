//! PRD-mcphost-agent-inbox
//! AC2 (P0) — Given the thread from AC1, When B calls
//! `host.msg.reply(thread_id, body="pong")` and A reads
//! `host.msg.thread(thread_id)`, Then A sees both messages ordered seq 1,
//! 2 with B's reply carrying `from_address` equal to B's namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac2_reply_appends_with_next_seq() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "ping"}))
        .await
        .expect("A sends to B");
    let send = extract_structured(&send_raw);
    let thread_id = send["thread_id"].as_str().unwrap().to_string();

    let reply_raw = client_b
        .tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": "pong"}))
        .await
        .expect("B replies");
    let reply = extract_structured(&reply_raw);
    assert_eq!(reply["seq"], json!(2), "{reply:?}");

    let thread_raw = client_a
        .tools_call("host.msg.thread", json!({"thread_id": thread_id}))
        .await
        .expect("A reads thread");
    let thread = extract_structured(&thread_raw);
    let messages = thread["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "{thread:?}");
    assert_eq!(messages[0]["seq"], json!(1), "{thread:?}");
    assert_eq!(messages[0]["from_address"], json!(ns_a), "{thread:?}");
    assert_eq!(messages[1]["seq"], json!(2), "{thread:?}");
    assert_eq!(messages[1]["from_address"], json!(ns_b), "{thread:?}");
}
