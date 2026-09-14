//! PRD-mcphost-agent-inbox
//! AC3 (P0) — Given tenant C is not a participant of that thread, When C
//! calls `host.msg.thread(thread_id)` or `host.msg.reply(thread_id, …)`,
//! Then both return `thread_not_found` byte-identical to the response for
//! a random nonexistent id.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

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
