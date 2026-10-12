//! PRD-mcphost-webhook-inbox-one-call
//! AC2 -- Given the inbox from AC1, When a POST signed with `secret`
//! carrying `{"id":"evt-fixture-1"}` reaches `url`, Then within 5 s
//! `host.tool_call(name="inbox")` returns a row with id `evt-fixture-1` and
//! `host.msg.wait` with `wait.args` returns a message whose `data.event_id`
//! is `evt-fixture-1`.

use crate::common;
use crate::whin;

use common::extract_structured;
use mcphost::sandbox;
use serde_json::{Value, json};

fn event_ids(read: &Value) -> Vec<String> {
    let rows = read
        .get("rows")
        .or_else(|| read.get("result").and_then(|r| r.get("rows")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rows.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect()
}

#[tokio::test]
async fn signed_post_is_a_tool_call_row_and_a_msg_wait_message() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fx = whin::start().await;
    let created = fx.create_inbox().await;
    let url = created["url"].as_str().expect("url").to_string();
    let secret = created["secret"].as_str().expect("secret").to_string();

    // The python environment may still be building on the first call; that
    // wait is the sandbox's, not the delivery's, so only the post-ready
    // round trips are held to the 5 s bound. The post + first read together
    // must finish within 30 s.
    let first = crate::deadline_budget!(30_000, {
        let resp = fx.post_signed(&url, &secret, br#"{"id":"evt-fixture-1"}"#).await;
        assert_eq!(resp.status(), 200, "signed delivery must be accepted");
        fx.read("inbox").await
    });
    assert!(event_ids(&first).contains(&"evt-fixture-1".to_string()), "tool_call rows: {first}");

    let wait = fx
        .client
        .tools_call("host.msg.wait", created["wait"]["args"].clone())
        .await
        .map(|v| extract_structured(&v))
        .expect("msg.wait");
    let messages = wait["messages"].as_array().unwrap_or_else(|| panic!("messages shape: {wait}"));
    assert_eq!(messages.len(), 1, "{wait}");
    assert_eq!(messages[0]["data"]["event_id"], json!("evt-fixture-1"), "{wait}");
    assert_eq!(messages[0]["data"]["tool"], json!("inbox"), "{wait}");
    assert_eq!(messages[0]["from_address"], json!("@host"), "{wait}");
}
