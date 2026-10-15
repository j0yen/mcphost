//! PRD-mcphost-sse-frame-contract-generated AC1 (P0, the falsifier) -- Given
//! an in-process server and 100 key-less `tools/call` requests each on a
//! fresh connection, When the bodies are split into frames, Then all 100 are
//! `text/event-stream` with methods `[notifications/tools/list_changed,
//! result]` in that order, and the counts are printed.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::protocol::frames::{FrameKind, observed_methods, split_sse_frames};
use serde_json::json;
use std::collections::BTreeMap;

const RUNS: usize = 100;

#[tokio::test]
async fn hundred_keyless_calls_all_answer_list_changed_then_result_on_an_event_stream() {
    let server = TestServer::start_with_signup_rate_limit(10_000).await;
    let mut by_shape: BTreeMap<String, usize> = BTreeMap::new();
    let mut offenders = Vec::new();

    for run in 0..RUNS {
        let client = McpClient::new(&server.base_url);
        let body = json!({
            "jsonrpc": "2.0",
            "id": run + 1,
            "method": "tools/call",
            "params": {
                "name": "host.tool_publish",
                "arguments": {
                    "name": format!("sfc{run}"),
                    "kind": "echo",
                    "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
                },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {"name": "sfc-ac01", "version": "0.1.0"},
                },
            },
        });
        let resp = client.post_raw(body).await;
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let text = resp.text().await.expect("read body");
        let methods = observed_methods(&split_sse_frames(&text));
        *by_shape.entry(format!("{content_type} {methods:?}")).or_default() += 1;

        let want = [FrameKind::ListChanged.method(), FrameKind::Result.method()];
        if content_type != "text/event-stream" || methods != want {
            offenders.push(format!("run {run}: {content_type} {methods:?}"));
        }
    }

    for (shape, count) in &by_shape {
        println!("sfc_ac01 count={count} shape={shape}");
    }
    assert!(
        offenders.is_empty(),
        "the key-less tools/call frame sequence is transport weather, not a contract ({} of {RUNS} differ): {}",
        offenders.len(),
        offenders.join("; ")
    );
    assert_eq!(by_shape.values().sum::<usize>(), RUNS);
}
