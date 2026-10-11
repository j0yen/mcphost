//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC3 (P0) -- Given a cursor minted before a process restart, When passed to
//! `host.msg.inbox`, Then `cursor_invalid` with `data.reason: "signature"` and
//! the remedy; Given a random string, Then `reason: "encoding"`.
//!
//! A second `TestServer` is a second process-secret: the same tenant id, the
//! same keyset position, a different HMAC key -- exactly what a restart does.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

/// Sign up sender + recipient (so the recipient's tenant id is the same on
/// every server this is called on) and return the recipient's client + address.
async fn boot() -> (TestServer, McpClient, McpClient, String) {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let (_ns_a, key_a) = signup(&server.base_url, "Pgtr AC3 Sender").await;
    let (ns_b, key_b) = signup(&server.base_url, "Pgtr AC3 Recipient").await;
    let sender = McpClient::with_bearer(&server.base_url, &key_a);
    let recipient = McpClient::with_bearer(&server.base_url, &key_b);
    (server, sender, recipient, ns_b)
}

#[tokio::test]
async fn inbox_refuses_a_cursor_from_a_previous_process_and_a_random_string() {
    let (_before, sender, recipient, to) = boot().await;
    for body in ["one", "two", "three"] {
        sender
            .tools_call("host.msg.send", json!({"to": [to], "body": body}))
            .await
            .expect("send");
    }
    let page = extract_structured(
        &recipient.tools_call("host.msg.inbox", json!({"limit": 1})).await.expect("inbox"),
    );
    let stale = page["next_cursor"].as_str().expect("a non-last page has next_cursor").to_string();

    // "Restart": a fresh server (fresh per-process secret), same tenant id.
    let (_after, _sender, recipient, _to) = boot().await;

    let err = recipient
        .tools_call("host.msg.inbox", json!({"cursor": stale}))
        .await
        .expect_err("a cursor from before the restart must be refused");
    assert_eq!(err.error_code.as_deref(), Some("cursor_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("signature"), "{err:?}");
    assert_eq!(err.data["remedy"], json!("omit cursor to restart from the first page"), "{err:?}");
    assert_eq!(err.message, "cursor is not valid (signature)", "{err:?}");

    let err = recipient
        .tools_call("host.msg.inbox", json!({"cursor": "not a cursor!"}))
        .await
        .expect_err("a random string must be refused");
    assert_eq!(err.error_code.as_deref(), Some("cursor_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("encoding"), "{err:?}");
    assert_eq!(err.data["remedy"], json!("omit cursor to restart from the first page"), "{err:?}");

    // The remedy works: omitting the cursor restarts from the first page.
    let restarted = extract_structured(&recipient.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    assert!(restarted["messages"].is_array(), "{restarted:?}");
}

#[tokio::test]
async fn inbox_walk_ends_with_no_next_cursor_key_and_thread_and_wait_share_the_cursor() {
    let (_server, sender, recipient, to) = boot().await;
    let mut thread_id = String::new();
    for body in ["one", "two", "three"] {
        let sent = extract_structured(
            &sender
                .tools_call("host.msg.send", json!({"to": [to], "body": body}))
                .await
                .expect("send"),
        );
        if thread_id.is_empty() {
            thread_id = sent["thread_id"].as_str().unwrap_or_default().to_string();
        }
    }

    let mut seen = 0;
    let mut cursor: Option<String> = None;
    let mut calls = 0;
    loop {
        let mut args = json!({"limit": 2});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(&recipient.tools_call("host.msg.inbox", args).await.expect("inbox"));
        calls += 1;
        seen += page["messages"].as_array().expect("messages").len();
        match page.get("next_cursor") {
            Some(c) => cursor = Some(c.as_str().expect("string cursor").to_string()),
            None => break,
        }
        assert!(calls < 5, "walk did not terminate");
    }
    assert_eq!((calls, seen), (2, 3), "3 messages at limit 2 = exactly 2 calls");

    // thread: a bad cursor is the same typed refusal.
    let err = recipient
        .tools_call("host.msg.thread", json!({"thread_id": thread_id, "cursor": "1"}))
        .await
        .expect_err("a bare integer is not a cursor any more");
    assert_eq!(err.error_code.as_deref(), Some("cursor_invalid"), "{err:?}");
    // wait: same decoder.
    let err = recipient
        .tools_call("host.msg.wait", json!({"cursor": "garbage!", "timeout_s": 1}))
        .await
        .expect_err("wait shares the cursor");
    assert_eq!(err.data["reason"], json!("encoding"), "{err:?}");
}
