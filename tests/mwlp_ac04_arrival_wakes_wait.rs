//! PRD-mcphost-msg-wait-long-poll-contract
//! AC4 (P0) — Given a message arrives 1 s into a 10 s wait, When the call
//! returns, Then `timed_out: false`, `waited_ms < 2000`, and `messages` holds
//! that one row.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn message_arriving_mid_wait_returns_it_with_timed_out_false() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "mwlp ac04 recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "mwlp ac04 sender").await;

    let base_url = server.base_url.clone();
    let waiter = tokio::spawn(async move {
        let client = McpClient::with_bearer(&base_url, &key_r);
        extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 10})).await.expect("wait"))
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    McpClient::with_bearer(&server.base_url, &key_s)
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "mid-wait"}))
        .await
        .expect("send");

    let body = waiter.await.expect("waiter task");
    assert_eq!(body["timed_out"], json!(false), "{body}");
    assert!(body["waited_ms"].as_u64().is_some_and(|ms| ms < 2000), "{body}");
    assert_eq!(body["messages"].as_array().map(Vec::len), Some(1), "{body}");
}
