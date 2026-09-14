//! PRD-mcphost-agent-inbox
//! AC1 (P0) — Given tenants A and B with B's policy `open`, When A calls
//! `host.msg.send(to=[B.address], body="ping")`, Then the result has one
//! `message_id`, a `thread_id`, `delivered_to=[B.address]`, and B's
//! `host.msg.inbox()` returns that message with `from_address` equal to
//! A's namespace.
//! AC2 (P0) — Given the thread from AC1, When B calls
//! `host.msg.reply(thread_id, body="pong")` and A reads
//! `host.msg.thread(thread_id)`, Then A sees both messages ordered seq 1,
//! 2 with B's reply carrying `from_address` equal to B's namespace.
//! AC3 (P0) — Given tenant C is not a participant of that thread, When C
//! calls `host.msg.thread(thread_id)` or `host.msg.reply(thread_id, …)`,
//! Then both return `thread_not_found` byte-identical to the response for
//! a random nonexistent id.

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

#[tokio::test]
async fn ac3_non_participant_gets_thread_not_found_byte_identical_to_nonexistent() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let (_ns_c, key_c) = signup(&server.base_url, "Agent C").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);
    let _ = &key_b;

    let send_raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b], "body": "ping"}))
        .await
        .expect("A sends to B");
    let send = extract_structured(&send_raw);
    let thread_id = send["thread_id"].as_str().unwrap().to_string();

    let err_real = client_c
        .tools_call("host.msg.thread", json!({"thread_id": thread_id}))
        .await
        .expect_err("C is not a participant");
    let err_fake = client_c
        .tools_call("host.msg.thread", json!({"thread_id": "t_doesnotexist00000000"}))
        .await
        .expect_err("nonexistent thread id");
    assert_eq!(err_real.error_code.as_deref(), Some("thread_not_found"), "{err_real:?}");
    assert_eq!(err_real.message, err_fake.message);
    assert_eq!(err_real.data, err_fake.data);

    let reply_err = client_c
        .tools_call("host.msg.reply", json!({"thread_id": thread_id, "body": "hi"}))
        .await
        .expect_err("C cannot reply either");
    assert_eq!(reply_err.error_code.as_deref(), Some("thread_not_found"), "{reply_err:?}");
}
