//! PRD-mcphost-tools-call-progress-notifications
//! AC5 (P0) — Given the same server, When `host.probe.progress {steps: 1,
//! step_ms: 1000}` is called without a token, and a tool that never reports is
//! called once with a token and once without, Then all three responses are
//! `application/json` with zero notification frames, and the two bodies of the
//! non-reporting tool are byte-identical to each other after the request id is
//! normalised.

use crate::common;
use crate::tcpn;
use tcpn::call_with_token;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

/// Content type and raw body text of `resp`.
async fn raw(resp: reqwest::Response) -> (String, String) {
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    (content_type, resp.text().await.expect("read body"))
}

fn assert_plain_json(content_type: &str, body: &str) -> Value {
    assert!(content_type.starts_with("application/json"), "{content_type}: {body}");
    let value: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("body is one JSON message: {e}: {body}"));
    assert!(value.get("method").is_none(), "no notification frame: {body}");
    assert!(value.get("result").is_some(), "{body}");
    value
}

#[tokio::test]
async fn tcpn_ac05_no_token_paths_stay_plain_json() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "tcpn plain").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let echo = common::publish(&client, "hello", "echo", json!({"schema": {"type": "object"}})).await;
    assert!(echo.starts_with(&ns), "{echo}");

    let (ct, body) = raw(call_with_token(&client, "host.probe.progress", json!({"steps": 1, "step_ms": 1000}), None).await).await;
    assert_plain_json(&ct, &body);

    let (ct_a, body_a) = raw(call_with_token(&client, &echo, json!({"msg": "hi"}), Some(json!("t"))).await).await;
    let (ct_b, body_b) = raw(call_with_token(&client, &echo, json!({"msg": "hi"}), None).await).await;
    let mut a = assert_plain_json(&ct_a, &body_a);
    let mut b = assert_plain_json(&ct_b, &body_b);
    // The request id and the running `calls_per_day` meter are the only
    // fields that differ between any two consecutive calls.
    for body in [&mut a, &mut b] {
        body["id"] = json!(0);
        body["result"]["_meta"]["usage"] = json!(null);
    }
    assert_eq!(a.to_string(), b.to_string(), "a token on a tool that never reports changes nothing");
    assert_eq!(ct_a, ct_b);
}
