//! PRD-mcphost-webhook-inbox-one-call
//! AC3 -- Given an unsigned or wrongly signed POST, When it reaches `url`,
//! Then the response is 401, no row is stored, no run fires and no inbox
//! message is sent.

use crate::common;
use crate::whin;

use common::extract_structured;
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn unsigned_and_wrongly_signed_posts_are_401_and_leave_no_trace() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let fx = whin::start().await;
    let created = fx.create_inbox().await;
    let url = created["url"].as_str().expect("url").to_string();
    let secret = created["secret"].as_str().expect("secret").to_string();
    let body = br#"{"id":"evt-forged-1"}"#;

    let unsigned = fx.post(&url, body, None).await;
    assert_eq!(unsigned.status(), 401, "unsigned");
    let wrong = fx.post(&url, body, Some(whin::hmac_sha256_hex("not-the-secret", body))).await;
    assert_eq!(wrong.status(), 401, "wrong key");
    let tampered = fx.post(&url, br#"{"id":"evt-forged-2"}"#, Some(whin::hmac_sha256_hex(&secret, body))).await;
    assert_eq!(tampered.status(), 401, "signature of a different body");

    let rows = extract_structured(
        &fx.client
            .tools_call("host.state.query", json!({"table": "inbox_inbox"}))
            .await
            .expect("state.query"),
    );
    assert_eq!(rows["rows"].as_array().map(Vec::len), Some(0), "no row stored: {rows}");

    let runs = extract_structured(&fx.client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    assert_eq!(runs["runs"].as_array().map(Vec::len), Some(0), "no run fired: {runs}");

    let inbox = extract_structured(&fx.client.tools_call("host.msg.inbox", json!({})).await.expect("msg.inbox"));
    assert_eq!(inbox["messages"].as_array().map(Vec::len), Some(0), "no inbox message: {inbox}");
}
