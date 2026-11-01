//! PRD-mcphost-refusals-name-their-cause AC5 -- Given the `http request`
//! line, When any `/mcp` POST completes, Then the line carries
//! `rpc=<method|batch|"">`, `tool=<tools/call name|"">` and
//! `reply=ok|jsonrpc_error:<code>|http:<status>|batch` (response extension,
//! line at body end), a 413 adds `content_length=` and `limit=`, and a test
//! with one global buffered subscriber runs concurrent calls of different
//! reply classes and asserts each line by `tool=`.
//!
//! The subscriber is `tests/common/tracecap.rs`'s: ONE process-global
//! subscriber installed once per test binary (never a thread-scoped default),
//! reading the lines the server task emits on this test's own thread.

use crate::common;

use common::tracecap::Capture;
use common::{McpClient, TestServer};
use mcphost::state::MAX_REQUEST_BODY_BYTES;
use serde_json::{Value, json};
use std::time::Duration;

/// The `http request` line for `path` whose `tool` field is `tool`, waiting
/// for it: the line is emitted when the reply body ends, which can be a beat
/// after the client has the bytes.
async fn line_for(buf: &Capture, tool: &str, rpc: &str) -> Value {
    for _ in 0..200 {
        let found = buf.json_lines().into_iter().find(|v| {
            let f = &v["fields"];
            f["message"] == "http request" && f["path"] == "/mcp" && f["tool"] == tool && f["rpc"] == rpc
        });
        if let Some(line) = found {
            return line["fields"].clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no `http request` line with rpc={rpc:?} tool={tool:?}: {}", buf.text());
}

async fn http_request_lines(buf: &Capture) -> Vec<Value> {
    // Let any line still waiting for its body to end land.
    tokio::time::sleep(Duration::from_millis(200)).await;
    buf.json_lines()
        .into_iter()
        .filter(|v| v["fields"]["message"] == "http request" && v["fields"]["path"] == "/mcp")
        .map(|v| v["fields"].clone())
        .collect()
}

#[tokio::test]
async fn concurrent_calls_of_different_reply_classes_are_told_apart_by_tool() {
    let server = TestServer::start().await;
    let (ns, key) = common::signup(&server.base_url, "refusals ac5").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    let qualified = common::publish(&owner, "t_ac5", "echo", json!({"schema": {"type": "object"}})).await;
    assert_eq!(qualified, format!("{ns}.t_ac5"));

    let buf = common::tracecap::capture_json();
    let keyless = McpClient::new(&server.base_url);
    let (ok, missing, not_found) = tokio::join!(
        owner.tools_call(&qualified, json!({})),
        keyless.tools_call("t_ac5_keyless", json!({})),
        owner.tools_call("t_ac5_nobody_owns_this", json!({})),
    );
    ok.expect("the published tool answers");
    assert_eq!(missing.expect_err("keyless").error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(not_found.expect_err("unknown").code, -32002);

    let ok_line = line_for(&buf, &qualified, "tools/call").await;
    assert_eq!(ok_line["reply"], "ok", "{ok_line}");
    assert_eq!(ok_line["status"], 200);

    // The refusal that crosses `oauth_401_upgrade`'s rewrite keeps its labels
    // and its post-rewrite class.
    let missing_line = line_for(&buf, "t_ac5_keyless", "tools/call").await;
    assert_eq!(missing_line["reply"], "http:401", "{missing_line}");
    assert_eq!(missing_line["status"], 401);

    let not_found_line = line_for(&buf, "t_ac5_nobody_owns_this", "tools/call").await;
    assert_eq!(not_found_line["reply"], "jsonrpc_error:-32002", "{not_found_line}");
    assert_eq!(not_found_line["status"], 200);
    for line in [&ok_line, &missing_line, &not_found_line] {
        assert!(line.get("content_length").is_none() && line.get("limit").is_none(), "only a 413 adds sizes: {line}");
        assert!(line.get("duration_ms").is_some(), "{line}");
    }
}

#[tokio::test]
async fn a_413_adds_content_length_and_limit_and_other_methods_are_labelled() {
    let server = TestServer::start().await;
    let buf = common::tracecap::capture_json();
    let http = reqwest::Client::new();
    let post = |body: String| {
        http.post(format!("{}/mcp", server.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body)
            .send()
    };

    // 413: the body was never parsed, so rpc and tool are blank.
    let pre = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"p":""#;
    let suf = r#""}}"#;
    let big = format!("{pre}{}{suf}", "x".repeat(MAX_REQUEST_BODY_BYTES + 1 - pre.len() - suf.len()));
    let resp = post(big).await.expect("send");
    assert_eq!(resp.status().as_u16(), 413);
    let line = line_for(&buf, "", "").await;
    assert_eq!(line["reply"], "http:413", "{line}");
    assert_eq!(line["content_length"], MAX_REQUEST_BODY_BYTES + 1);
    assert_eq!(line["limit"], MAX_REQUEST_BODY_BYTES);
    assert_eq!(line["status"], 413);

    // A batch logs `rpc=batch`; a 200 reply is `reply=batch`, any other status `http:<status>`
    // (this transport answers an ordinary batch 415). Gate-refused methods in a batch are
    // answered by `refuse_unimplemented_methods` itself.
    let batch = json!([
        {"jsonrpc": "2.0", "id": 1, "method": "roots/list", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "roots/list", "params": {}},
    ]);
    let resp = post(batch.to_string()).await.expect("send batch");
    let status = resp.status().as_u16();
    let line = line_for(&buf, "", "batch").await;
    let expected = if status == 200 { "batch".to_string() } else { format!("http:{status}") };
    assert_eq!(line["reply"], expected, "{line}");
    assert!(line.get("content_length").is_none(), "{line}");

    // A notification is answered with a bare status, logged as `http:<status>`.
    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let resp = post(note.to_string()).await.expect("send notification");
    let status = resp.status().as_u16();
    let line = line_for(&buf, "", "notifications/initialized").await;
    assert_eq!(line["reply"], format!("http:{status}"), "{line}");

    // Every /mcp POST line carries all three keys.
    for line in http_request_lines(&buf).await {
        for key in ["rpc", "tool", "reply"] {
            assert!(line.get(key).is_some(), "{key} missing on {line}");
        }
    }
}

#[tokio::test]
async fn a_keyless_signup_reply_that_upgrades_to_sse_is_still_classified_ok() {
    let server = TestServer::start().await;
    let buf = common::tracecap::capture_json();
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host.whoami","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"refusals-ac05","version":"1"}}}}"#;
    let resp = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "host.whoami")
        .body(body)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status().as_u16(), 200);
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let body = resp.text().await.expect("body");
    assert!(
        content_type.starts_with("text/event-stream"),
        "a sign-up reply is the SSE upgrade carrying list_changed then the result: {content_type} {body}"
    );
    let line = line_for(&buf, "host.whoami", "tools/call").await;
    assert_eq!(line["reply"], "ok", "{line}");
}
