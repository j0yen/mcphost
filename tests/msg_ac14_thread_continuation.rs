//! PRD-mcphost-agent-inbox
//! AC14 (P1) — Given A is a participant in thread T, When A calls
//! `host.msg.send(thread_id=T, to=[C.address], body=…)`, Then C becomes a
//! participant, sees the new message in its inbox, and does not see
//! messages with `seq` earlier than that one in `host.msg.inbox()` (but
//! does in `host.msg.thread(T)`).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac14_adding_a_recipient_to_an_existing_thread_joins_them_going_forward() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Agent C").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);
    let _ = &ns_a;

    let send1_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "seq 1"}))
        .await
        .expect("A starts a thread with B");
    let send1 = extract_structured(&send1_raw);
    let thread_id = send1["thread_id"].as_str().unwrap().to_string();

    let reply_raw = client_b
        .tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": "seq 2"}))
        .await
        .expect("B replies");
    let reply = extract_structured(&reply_raw);
    assert_eq!(reply["seq"], json!(2), "{reply:?}");

    let send3_raw = client_a
        .tools_call(
            "host.msg.send",
            json!({"thread_id": thread_id, "to": [ns_c.clone()], "body": "seq 3, welcome C"}),
        )
        .await
        .expect("A adds C to the thread");
    let send3 = extract_structured(&send3_raw);
    assert_eq!(send3["thread_id"], json!(thread_id), "{send3:?}");
    assert_eq!(send3["delivered_to"], json!([ns_c]), "{send3:?}");
    let seq3 = send3["seq"].as_i64().unwrap();

    // C sees the new message in its inbox, and nothing earlier.
    let c_inbox_raw = client_c.tools_call("host.msg.inbox", json!({})).await.expect("C inbox");
    let c_inbox = extract_structured(&c_inbox_raw);
    let c_messages = c_inbox["messages"].as_array().expect("messages array");
    assert_eq!(c_messages.len(), 1, "{c_inbox:?}");
    assert_eq!(c_messages[0]["seq"], json!(seq3), "{c_inbox:?}");

    // C reading the whole thread DOES see the earlier messages.
    let c_thread_raw = client_c
        .tools_call("host.msg.thread", json!({"thread_id": thread_id}))
        .await
        .expect("C reads the thread");
    let c_thread = extract_structured(&c_thread_raw);
    let c_thread_messages = c_thread["messages"].as_array().expect("messages array");
    assert_eq!(c_thread_messages.len(), 3, "{c_thread:?}");
    assert_eq!(c_thread_messages[0]["seq"], json!(1), "{c_thread:?}");
    assert_eq!(c_thread_messages[2]["seq"], json!(seq3), "{c_thread:?}");
}
