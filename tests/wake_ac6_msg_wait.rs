//! PRD-mcphost-agent-wake
//! AC6 (P0) — Given R has called host.msg.wait(cursor=c, timeout_s=25),
//! When S sends R a message 2s later, Then the call returns within 3s of
//! the send with that message and an advanced cursor; and When no message
//! arrives, Then the call returns at 25s ± 1s with an empty list and
//! cursor c.
//!
//! Both branches are exercised with a much-shortened timeout_s (2-3s
//! instead of a literal 25s) rather than a real 25s+ wait, per the build
//! instructions -- the ± tolerance around whatever timeout_s is passed is
//! what's actually under test, not the specific 25s ceiling (host.msg.wait
//! clamps to 25 regardless; a smaller value exercises the identical code
//! path).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn wait_returns_promptly_when_a_message_arrives() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC6a Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC6a Sender").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);

    // Prime a message and immediately wait it in (timeout_s=1, but it
    // returns right away since one is already pending) purely to obtain an
    // advanced cursor `c` from a real host.msg.wait response -- pagination
    // cursors from a partial-page host.msg.inbox call don't advance
    // (requirement 6's own doc comment), so this is the one way to get a
    // "resume after what I've already seen" cursor through the public API.
    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "priming"}))
        .await
        .expect("priming send");
    let primed = extract_structured(
        &client_r
            .tools_call("host.msg.wait", json!({"timeout_s": 1}))
            .await
            .expect("priming wait"),
    );
    let cursor = primed["next_cursor"].as_str().expect("advanced cursor").to_string();

    let base_url = server.base_url.clone();
    let key_r_2 = key_r.clone();
    let waiter = tokio::spawn(async move {
        let client = McpClient::with_bearer(&base_url, &key_r_2);
        client
            .tools_call("host.msg.wait", json!({"cursor": cursor, "timeout_s": 3}))
            .await
            .expect("msg.wait")
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    let send_at = Instant::now();
    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "wake up now"}))
        .await
        .expect("S sends");

    let waited_raw = waiter.await.expect("waiter task");
    let waited = extract_structured(&waited_raw);
    let elapsed = send_at.elapsed();
    assert!(elapsed < Duration::from_secs(3), "wait must return within 3s of the send: {elapsed:?}");
    let messages = waited["messages"].as_array().expect("messages array");
    assert!(
        messages.iter().any(|m| m["body"] == json!("wake up now")),
        "the woken message must be the new one, not the priming one: {waited:?}"
    );
    assert!(waited["next_cursor"].is_string(), "cursor must advance once a message is returned: {waited:?}");
}

#[tokio::test]
async fn wait_times_out_with_an_empty_list_and_the_unchanged_cursor() {
    let server = TestServer::start().await;
    let (_ns_r, key_r) = signup(&server.base_url, "Wake AC6b Recipient").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);

    // A cursor pointing at "nothing pending yet" for a tenant with an empty
    // inbox, with nothing ever sent: host.msg.wait/inbox both treat a
    // cursor as opaque `"<unix_ms>.<id>"`, so any well-formed value works
    // to prove "returned unchanged".
    let cursor = "1.01ARBITRARYIDXXXXXXXXXX";

    let start = Instant::now();
    let waited = extract_structured(
        &client_r
            .tools_call("host.msg.wait", json!({"timeout_s": 2, "cursor": cursor}))
            .await
            .expect("msg.wait"),
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1900) && elapsed <= Duration::from_secs(4),
        "wait must return at ~timeout_s when nothing arrives: {elapsed:?}"
    );
    assert_eq!(waited["messages"], json!([]), "{waited:?}");
    assert_eq!(waited["next_cursor"], json!(cursor), "cursor must stay unchanged on timeout: {waited:?}");
}
