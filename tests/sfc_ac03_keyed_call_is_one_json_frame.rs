//! PRD-mcphost-sse-frame-contract-generated AC3 (P0) -- Given a keyed
//! `tools/call` of `host.whoami`, When the response is read, Then it is
//! `application/json` with exactly one frame carrying `result`, and the row
//! for `tools/call:keyed` allows exactly one `result` (and an optional
//! leading `list_changed`).

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::protocol::frames::{Card, FrameKind, row_for};
use serde_json::{Value, json};

#[tokio::test]
async fn keyed_whoami_answers_one_json_result_frame_and_the_row_says_so() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SFC AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let resp = client
        .post_raw(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "host.whoami",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {"name": "sfc-ac03", "version": "0.1.0"},
                },
            },
        }))
        .await;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("application/json"), "keyed call must be plain JSON, got {content_type}");
    let body: Value = resp.json().await.expect("one JSON document");
    assert!(body.get("result").is_some(), "the single frame must carry `result`: {body}");
    assert!(body.get("method").is_none(), "the single frame must not be a notification: {body}");

    let row = row_for("tools/call:keyed").expect("row for tools/call:keyed");
    // PRD-mcphost-notes-to-future-self: `host.note.put`/`delete` on a tool may
    // precede the result with `list_changed`; `host.whoami` never does.
    assert_eq!(
        row.frames,
        &[(FrameKind::ListChanged, Card::AtMostOne), (FrameKind::Result, Card::ExactlyOne)],
        "the row must say [ListChanged{{0,1}}, Result{{1}}]"
    );
}
