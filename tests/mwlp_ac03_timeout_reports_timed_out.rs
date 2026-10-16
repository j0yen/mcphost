//! PRD-mcphost-msg-wait-long-poll-contract
//! AC3 (P0) — Given a keyed tenant with an empty inbox, When
//! `host.msg.wait timeout_s=2` runs, Then it returns after 2,000-2,600 ms
//! with `timed_out: true`, `waited_ms` in that range, `timeout_s_applied: 2`,
//! `next_cursor` unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Instant;

#[tokio::test]
async fn empty_inbox_wait_reports_timed_out_waited_ms_and_applied_timeout() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "mwlp ac03 tenant").await;
    let (_ns_s, key_s) = signup(&server.base_url, "mwlp ac03 sender").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let sender = McpClient::with_bearer(&server.base_url, &key_s);

    // Without a cursor the key stays absent (paged.rs: absent, never null).
    let started = Instant::now();
    let body = extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 2})).await.expect("wait"));
    let elapsed = started.elapsed().as_millis();
    assert!((2000..=2600).contains(&elapsed), "wall {elapsed} ms: {body}");
    assert_eq!(body["timed_out"], json!(true), "{body}");
    assert_eq!(body["messages"], json!([]), "{body}");
    assert_eq!(body["timeout_s_applied"], json!(2), "{body}");
    let waited = body["waited_ms"].as_u64().expect("waited_ms");
    assert!((2000..=2600).contains(&waited), "waited_ms {waited}: {body}");
    assert!(body.get("next_cursor").is_none(), "{body}");

    // With a cursor it comes back unchanged. Obtain a real one from a hit.
    sender.tools_call("host.msg.send", json!({"to": [ns], "body": "prime"})).await.expect("send");
    let primed = extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 1})).await.expect("prime"));
    assert_eq!(primed["timed_out"], json!(false), "{primed}");
    let cursor = primed["next_cursor"].as_str().expect("cursor").to_string();
    let body = extract_structured(
        &client
            .tools_call("host.msg.wait", json!({"cursor": cursor, "timeout_s": 2}))
            .await
            .expect("wait with cursor"),
    );
    assert_eq!(body["timed_out"], json!(true), "{body}");
    assert_eq!(body["next_cursor"], json!(cursor), "{body}");

    // Above the ceiling: clamped and reported (R6).
    let clamped = extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 1})).await.expect("wait"));
    assert_eq!(clamped["timeout_s_applied"], json!(1), "{clamped}");
}
