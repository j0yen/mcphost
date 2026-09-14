//! PRD-mcphost-agent-inbox
//! AC10 (P0) — Given a body of 16 KiB + 1 byte, or `to` with 6 addresses
//! on a plan whose cap is 5, When sent, Then each returns `quota_exceeded`
//! naming `msg_body_bytes_max` or `recipients_per_msg_max` respectively
//! and stores nothing.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac10_oversized_body_is_refused_and_stores_nothing() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let oversized_body = "x".repeat(16 * 1024 + 1);
    let err = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": oversized_body}))
        .await
        .expect_err("oversized body must be refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!("msg_body_bytes_max"), "{err:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
}

#[tokio::test]
async fn ac10_too_many_recipients_is_refused_and_stores_nothing() {
    // 7 signups (1 sender + 6 recipients) need a higher-than-default rate
    // limit, same convention as e.g. agentdir_ac05_ac06_search.rs.
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    let mut recipients = Vec::new();
    let mut recipient_keys = Vec::new();
    for i in 0..6 {
        let (ns, key) = signup(&server.base_url, &format!("Recipient {i}")).await;
        recipients.push(ns);
        recipient_keys.push(key);
    }

    let err = client_a
        .tools_call("host.msg.send", json!({"to": recipients.clone(), "body": "ping"}))
        .await
        .expect_err("6 recipients over the cap of 5 must be refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!("recipients_per_msg_max"), "{err:?}");
    assert_eq!(err.data["value"], json!(5), "{err:?}");

    for key in &recipient_keys {
        let client = McpClient::with_bearer(&server.base_url, key);
        let inbox_raw = client.tools_call("host.msg.inbox", json!({})).await.expect("inbox");
        let inbox = extract_structured(&inbox_raw);
        assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
    }
}
