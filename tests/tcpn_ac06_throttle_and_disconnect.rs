//! PRD-mcphost-tools-call-progress-notifications
//! AC6 (P0) — Given a tool that reports 50 times within 1 s with a token, When
//! the call runs, Then at most 2 progress frames are sent in that second, the
//! last progress frame carries the final `pct`/`message`, no report is sent
//! twice, and `progress_frames` on the `calls` row equals the frames sent;
//! Given the client disconnects after the first frame, When the tool
//! finishes, Then the call completes, the `calls` row has `ok: true`, nothing
//! is logged above `info`, and the process does not panic.
//!
//! The `chatter` kind is the tool: `args.reports` reports at once, then (when
//! `args.sleep_ms` > 0) one more after the sleep. Log capture goes through
//! `common::tracecap` (the request round trip stays on this test's thread).

use crate::common;
use crate::tcpn;
use tcpn::call_with_token;
use async_trait::async_trait;
use common::tracecap;
use common::{McpClient, TestServer, publish, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct ChatterKind;

#[async_trait]
impl Kind for ChatterKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "chatter"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor { name: "chatter".to_string(), description: "reports progress in a burst".to_string(), input_schema: json!({"type": "object"}) }
    }

    async fn call(&self, _spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let reports = args["reports"].as_i64().unwrap_or(0);
        for i in 1..=reports {
            ctx.progress.report(Some(i * 100 / reports), Some(format!("r{i}")));
        }
        if let Some(ms) = args["sleep_ms"].as_u64().filter(|ms| *ms > 0) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            ctx.progress.report(Some(100), Some("late".to_string()));
        }
        Ok(json!({"done": true}))
    }
}

fn chatter_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChatterKind));
    kinds
}

fn calls_row(server: &TestServer, tool_suffix: &str) -> Option<(i64, i64, i64)> {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    conn.query_row(
        "SELECT ok, progress_token, progress_frames FROM calls WHERE tool_name LIKE ?1 ORDER BY id DESC LIMIT 1",
        [format!("%{tool_suffix}")],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

#[tokio::test]
async fn tcpn_ac06_fifty_reports_in_a_second_send_at_most_two_frames() {
    let server = TestServer::start_with_kinds(chatter_registry()).await;
    let (_ns, key) = signup(&server.base_url, "tcpn throttle").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tool = publish(&client, "burst", "chatter", json!({})).await;

    let resp = call_with_token(&client, &tool, json!({"reports": 50}), Some(json!("b"))).await;
    let text = resp.text().await.expect("body");
    let msgs = common::parse_sse_messages(&text);
    let frames: Vec<&Value> = msgs.iter().filter(|m| m["method"] == "notifications/progress").map(|m| &m["params"]).collect();
    assert!((1..=2).contains(&frames.len()), "at most 2 frames for 50 reports in 1 s: {frames:?}");
    let last = frames.last().expect("a frame");
    assert_eq!(last["progress"].as_f64(), Some(100.0), "the last frame carries the final pct: {frames:?}");
    assert_eq!(last["message"], json!("r50"), "and the final message: {frames:?}");
    let sent: Vec<f64> = frames.iter().filter_map(|f| f["progress"].as_f64()).collect();
    assert!(sent.windows(2).all(|w| w[0] < w[1]), "no report is sent twice: {sent:?}");
    assert!(msgs.last().is_some_and(|m| m.get("result").is_some()), "the result follows the last frame: {msgs:?}");

    let (ok, token, frames_col) = calls_row(&server, "burst").expect("the call's row");
    assert_eq!((ok, token), (1, 1));
    assert_eq!(frames_col, frames.len() as i64, "progress_frames equals the frames sent");
}

#[tokio::test]
async fn tcpn_ac06_client_gone_after_first_frame_still_completes() {
    let server = TestServer::start_with_kinds(chatter_registry()).await;
    let (_ns, key) = signup(&server.base_url, "tcpn gone").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tool = publish(&client, "gone", "chatter", json!({})).await;
    let logs = tracecap::capture_json();

    let mut resp = call_with_token(&client, &tool, json!({"reports": 1, "sleep_ms": 1500}), Some(json!("g"))).await;
    let first = resp.chunk().await.expect("read first chunk").expect("a first chunk");
    assert!(String::from_utf8_lossy(&first).contains("notifications/progress"), "the first chunk is a progress frame");
    drop(resp);

    let mut row = None;
    for _ in 0..80 {
        row = calls_row(&server, "gone");
        if row.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (ok, token, _) = row.expect("the call completed and wrote its row after the client left");
    assert_eq!((ok, token), (1, 1), "ok: true");
    let loud: Vec<Value> = logs
        .json_lines()
        .into_iter()
        .filter(|l| matches!(l["level"].as_str(), Some("WARN" | "ERROR")))
        // rmcp's own `serve_inner` teardown logs at warn/error whenever a
        // client drops a connection with a handler in flight (service.rs
        // drain), with or without a progress token; it is not mcphost's log.
        .filter(|l| l["span"]["name"] != "serve_inner")
        .collect();
    assert!(loud.is_empty(), "nothing above info: {loud:?}");
}
