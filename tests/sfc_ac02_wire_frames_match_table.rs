//! PRD-mcphost-sse-frame-contract-generated AC2 (P0) -- Given every row of
//! `FRAME_SEQUENCES`, When the drift test sends that request type to an
//! in-process server, Then the observed frame methods match the row's
//! cardinality pattern and the content type matches `json_when_single_frame`.
//!
//! Frames are split exactly as `compat_check.rs` splits them (`\n\n` blocks,
//! `data:` lines) via `frames::split_sse_frames`, so parser and test cannot
//! diverge. A row with no request below fails the test naming it.
//!
//! The `server_timing_t*` tests below pin the `Server-Timing: app;dur=<ms>`
//! header on `/mcp` (hotfix, no PRD): T1 JSON response, T2 SSE response, T3
//! refusals before and after dispatch (the layer is the outermost route layer).

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::protocol::frames::{
    FRAME_SEQUENCES, FrameRow, SERVER_TIMING_HEADER, SERVER_TIMING_METRIC, body_matches_row, observed_methods, split_sse_frames,
};
use serde_json::{Value, json};

const META: &str = "io.modelcontextprotocol/";

fn meta() -> Value {
    json!({
        format!("{META}protocolVersion"): "2026-07-28",
        format!("{META}clientCapabilities"): {},
        format!("{META}clientInfo"): {"name": "sfc-ac02", "version": "0.1.0"},
    })
}

fn request(id: u64, method: &str, mut params: Value) -> Value {
    if method != "initialize"
        && let Some(obj) = params.as_object_mut()
    {
        obj.insert("_meta".to_string(), meta());
    }
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Send the request type of `row` and return `(content_type, frames)`.
async fn observe(server: &TestServer, key: &str, row: &FrameRow) -> Option<(String, Vec<Value>)> {
    let anon = McpClient::new(&server.base_url);
    let keyed = McpClient::with_bearer(&server.base_url, key);
    let resp = match row.request_type {
        "initialize" => {
            let params = json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "sfc-ac02", "version": "0.1.0"},
            });
            anon.post_raw(request(1, "initialize", params)).await
        }
        "tools/list" => keyed.post_raw(request(2, "tools/list", json!({}))).await,
        "ping" => keyed.post_raw(request(3, "ping", json!({}))).await,
        "tools/call:keyed" => keyed.post_raw(request(4, "tools/call", json!({"name": "host.whoami", "arguments": {}}))).await,
        "tools/call:keyless_signup" => {
            let args = json!({"name": "sfcac02", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}, "required": []}}});
            anon.post_raw(request(5, "tools/call", json!({"name": "host.tool_publish", "arguments": args}))).await
        }
        "tools/call:error" => {
            keyed.post_raw(request(6, "tools/call", json!({"name": "host.run_snippet", "arguments": {}}))).await
        }
        _ => return None,
    };
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
    let frames = if content_type == "text/event-stream" {
        split_sse_frames(&text)
    } else {
        vec![serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: parse JSON body: {e}: {text}", row.request_type))]
    };
    Some((content_type, frames))
}

#[tokio::test]
async fn every_row_of_the_table_matches_the_real_wire() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SFC AC2").await;
    let mut failures = Vec::new();
    for row in FRAME_SEQUENCES {
        let Some((content_type, frames)) = observe(&server, &key, row).await else {
            failures.push(format!("{}: the drift test has no request for this row", row.request_type));
            continue;
        };
        let methods = observed_methods(&frames);
        println!("sfc_ac02 {} -> {content_type} {methods:?}", row.request_type);
        if !body_matches_row(row, &frames) {
            failures.push(format!("{}: observed frames {methods:?} do not match the row", row.request_type));
        }
        let want = row.content_type_rule.content_type_for(frames.len());
        if content_type != want {
            failures.push(format!("{}: content type {content_type} but {} wants {want} for {} frame(s)", row.request_type, row.content_type_rule.name(), frames.len()));
        }
    }
    assert!(failures.is_empty(), "wire drifted from FRAME_SEQUENCES:\n{}", failures.join("\n"));
}

/// The header's `dur` in ms, after checking the metric name is the const.
fn dur_ms(resp: &reqwest::Response) -> f64 {
    let raw = resp
        .headers()
        .get(SERVER_TIMING_HEADER)
        .unwrap_or_else(|| panic!("no {SERVER_TIMING_HEADER} header on {} response: {:?}", resp.status(), resp.headers()))
        .to_str()
        .expect("header is ASCII");
    let (name, param) = raw.split_once(';').unwrap_or_else(|| panic!("no `;` in {raw:?}"));
    assert_eq!(name, SERVER_TIMING_METRIC, "{raw:?}");
    let dur = param.strip_prefix("dur=").unwrap_or_else(|| panic!("no dur= in {raw:?}"));
    let ms: f64 = dur.parse().unwrap_or_else(|e| panic!("dur {dur:?} is not a number: {e}"));
    assert!(ms.is_finite() && ms >= 0.0, "dur must be finite and >= 0: {raw:?}");
    ms
}

#[tokio::test]
async fn server_timing_t1_json_mcp_response_carries_app_dur_and_healthz_does_not() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Stim T1").await;

    let init = json!({"protocolVersion": "2026-07-28", "capabilities": {}, "clientInfo": {"name": "stim", "version": "0.1.0"}});
    let resp = McpClient::new(&server.base_url).post_raw(request(1, "initialize", init)).await;
    assert!(resp.status().is_success(), "{}", resp.status());
    dur_ms(&resp);

    let resp = McpClient::with_bearer(&server.base_url, &key).post_raw(request(2, "tools/list", json!({}))).await;
    assert!(resp.status().is_success(), "{}", resp.status());
    assert_eq!(
        resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.split(';').next().unwrap_or("")),
        Some("application/json")
    );
    dur_ms(&resp);

    let health = reqwest::get(format!("{}/healthz", server.base_url)).await.expect("healthz");
    assert!(health.headers().get(SERVER_TIMING_HEADER).is_none(), "only /mcp is timed");
}

#[tokio::test]
async fn server_timing_t2_sse_mcp_response_carries_the_header_at_response_head() {
    let server = TestServer::start().await;
    let args = json!({"name": "stimt2", "kind": "echo", "spec": {"schema": {"type": "object", "properties": {}, "required": []}}});
    let resp = McpClient::new(&server.base_url)
        .post_raw(request(5, "tools/call", json!({"name": "host.tool_publish", "arguments": args})))
        .await;
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    assert!(content_type.starts_with("text/event-stream"), "premise: a keyless publish streams, got {content_type:?}");
    dur_ms(&resp);
    let _ = resp.text().await;
}

#[tokio::test]
async fn server_timing_t3_refusals_before_and_after_dispatch_still_carry_the_header() {
    let server = TestServer::start().await;

    // A tenant-requiring call with no key is refused in the handler
    // (`tenant_key_missing`) and upgraded to 401 by the router-wide
    // `oauth_401_upgrade`, which runs outside the route layers.
    let resp = McpClient::new(&server.base_url)
        .post_raw(request(1, "tools/call", json!({"name": "host.state.get", "arguments": {"key": "k"}})))
        .await;
    assert_eq!(resp.status().as_u16(), 401, "keyless tenant call");
    dur_ms(&resp);

    // A bad key is refused in the handler too (status is whatever the handler
    // chooses); the header rides on it either way.
    let resp = McpClient::with_bearer(&server.base_url, "mch_not_a_real_key")
        .post_raw(request(2, "tools/call", json!({"name": "host.whoami", "arguments": {}})))
        .await;
    dur_ms(&resp);

    // The pre-dispatch method gate answers `roots/list` itself, before auth or
    // the service run: only a layer outside it times that response.
    let resp = reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": "roots/list", "params": {}}).to_string())
        .send()
        .await
        .expect("POST /mcp roots/list");
    dur_ms(&resp);
}
