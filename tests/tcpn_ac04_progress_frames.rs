//! PRD-mcphost-tools-call-progress-notifications
//! AC4 (P0) — Given an in-process server behind the streamable-HTTP tower
//! service in JSON-response mode, When `host.probe.progress {steps: 3,
//! step_ms: 1000}` is called with `_meta.progressToken: "t1"`, Then the
//! Content-Type is `text/event-stream`, exactly 3 `notifications/progress`
//! frames precede the result with `progressToken` `"t1"`, `progress`
//! 33/66/100, `total` 100 and `message` verbatim; a numeric token 7 echoes as
//! 7; `{steps: 2, step_ms: 1000, mode: "count"}` yields `progress` 1 then 2
//! with no `total`; and the `calls` row ends `progress_token=1,
//! progress_frames=3`.

use crate::common;
use crate::tcpn;
use tcpn::call_with_token;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

/// The response's content type and its decoded JSON-RPC messages, in order.
async fn messages(resp: reqwest::Response) -> (String, Vec<Value>) {
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let text = resp.text().await.expect("read body");
    let msgs = if content_type.starts_with("text/event-stream") { common::parse_sse_messages(&text) } else { vec![serde_json::from_str(&text).expect("json body")] };
    (content_type, msgs)
}

fn progress_frames(msgs: &[Value]) -> Vec<&Value> {
    msgs.iter().filter(|m| m["method"] == "notifications/progress").map(|m| &m["params"]).collect()
}

#[tokio::test]
async fn tcpn_ac04_token_call_streams_progress_before_result() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tcpn frames").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let resp = call_with_token(&client, "host.probe.progress", json!({"steps": 3, "step_ms": 1000}), Some(json!("t1"))).await;
    let (content_type, msgs) = messages(resp).await;
    assert!(content_type.starts_with("text/event-stream"), "{content_type}");
    let frames = progress_frames(&msgs);
    assert_eq!(frames.len(), 3, "{msgs:?}");
    let result_at = msgs.iter().position(|m| m.get("result").is_some()).expect("a result message");
    assert_eq!(result_at, msgs.len() - 1, "the result is the last message: {msgs:?}");
    for (frame, (progress, message)) in frames.iter().zip([(33, "step 1/3"), (66, "step 2/3"), (100, "step 3/3")]) {
        assert_eq!(frame["progressToken"], json!("t1"));
        assert_eq!(frame["progress"].as_f64(), Some(f64::from(progress)));
        assert_eq!(frame["total"].as_f64(), Some(100.0));
        assert_eq!(frame["message"], json!(message));
    }

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let row: (i64, i64) = conn
        .query_row(
            "SELECT progress_token, progress_frames FROM calls WHERE tool_name = 'host.probe.progress' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("the probe call's row");
    assert_eq!(row, (1, 3), "calls row progress_token / progress_frames");
}

#[tokio::test]
async fn tcpn_ac04_numeric_token_echoes_and_count_mode_has_no_total() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tcpn count").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let resp = call_with_token(&client, "host.probe.progress", json!({"steps": 1, "step_ms": 1000}), Some(json!(7))).await;
    let (_, msgs) = messages(resp).await;
    let frames = progress_frames(&msgs);
    assert_eq!(frames.len(), 1, "{msgs:?}");
    assert_eq!(frames[0]["progressToken"], json!(7), "a numeric token echoes as a number");

    let resp = call_with_token(&client, "host.probe.progress", json!({"steps": 2, "step_ms": 1000, "mode": "count"}), Some(json!("c"))).await;
    let (_, msgs) = messages(resp).await;
    let frames = progress_frames(&msgs);
    assert_eq!(frames.iter().map(|f| f["progress"].as_f64()).collect::<Vec<_>>(), vec![Some(1.0), Some(2.0)], "{msgs:?}");
    assert!(frames.iter().all(|f| f.get("total").is_none()), "count mode carries no total: {frames:?}");
}
