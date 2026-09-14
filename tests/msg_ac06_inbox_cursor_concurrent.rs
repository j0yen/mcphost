//! PRD-mcphost-agent-inbox
//! AC6 (P0) — Given 20 concurrent senders each sending 50 messages to B,
//! When B reads `host.msg.inbox(cursor, limit=37)` in a loop until
//! `next_cursor` stops advancing, Then B collected exactly 1,000 distinct
//! message ids with no duplicates.

use std::collections::HashSet;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac6_exactly_once_delivery_under_interleaved_sends() {
    // 21 signups (1 recipient + 20 senders) against one source IP need a
    // higher-than-default rate limit, same convention as e.g.
    // agentdir_ac05_ac06_search.rs.
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let base_url = server.base_url.clone();

    let mut senders = Vec::new();
    for i in 0..20 {
        let (_ns, key) = signup(&base_url, &format!("Sender {i}")).await;
        senders.push(key);
    }

    let mut tasks = tokio::task::JoinSet::new();
    for key in senders {
        let base_url = base_url.clone();
        let ns_b = ns_b.clone();
        tasks.spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            for n in 0..50 {
                client
                    .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("msg {n}")}))
                    .await
                    .expect("send under concurrency");
            }
        });
    }
    while tasks.join_next().await.is_some() {}

    let client_b = McpClient::with_bearer(&base_url, &key_b);
    let mut seen = HashSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({"limit": 37});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let raw = client_b.tools_call("host.msg.inbox", args).await.expect("B inbox page");
        let page = extract_structured(&raw);
        let messages = page["messages"].as_array().expect("messages array");
        for m in messages {
            let id = m["message_id"].as_str().unwrap().to_string();
            assert!(seen.insert(id.clone()), "duplicate message id {id}");
        }
        let next_cursor = page["next_cursor"].as_str().map(str::to_string);
        if next_cursor.is_none() || next_cursor == cursor {
            break;
        }
        cursor = next_cursor;
    }

    assert_eq!(seen.len(), 1_000, "expected exactly 1000 distinct messages, got {}", seen.len());
}
