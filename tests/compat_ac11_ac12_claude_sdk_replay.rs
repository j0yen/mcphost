//! AC11 — Given a client that reads `MCP-Protocol-Version` from the
//! `initialize` response and echoes it on the next request, When it calls
//! `tools/list` with that header and with neither an `Mcp-Method` header
//! nor a `_meta` object, Then it receives a successful result rather than
//! JSON-RPC `-32020` or `-32602`.
//! AC12 — Given a replay of the exact request sequence the Claude Agent
//! SDK emits -- `initialize` with `protocolVersion: "2025-11-25"`, then
//! `notifications/initialized`, then `tools/list` carrying the
//! `MCP-Protocol-Version` value taken from the `initialize` response
//! header, no SEP-2243 headers, no `_meta` -- When the sequence is run
//! against a `TestServer`, Then `tools/list` returns HTTP 200 with a
//! result that includes `signup`, whose `inputSchema` declares the
//! required property `name`.

mod common;
use common::TestServer;
use serde_json::Value;

/// A raw HTTP client deliberately independent of `McpClient`: the point of
/// both ACs here is to control the exact headers sent, and `McpClient`
/// always adds SEP-2243 `Mcp-Method`/`Mcp-Name` headers and a `_meta`
/// object on every non-initialize call (see `tests/common/mod.rs`), which
/// is precisely what these tests must NOT send.
async fn post(
    http: &reqwest::Client,
    base_url: &str,
    body: &Value,
    protocol_version_header: Option<&str>,
) -> reqwest::Response {
    let mut req = http
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(body);
    if let Some(v) = protocol_version_header {
        req = req.header("MCP-Protocol-Version", v);
    }
    req.send().await.expect("send request")
}

#[tokio::test]
async fn ac11_echoed_negotiated_version_is_accepted_with_no_sep2243_headers() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let init_resp = post(
        &http,
        &server.base_url,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "ac11-test", "version": "0.1"}
            }
        }),
        None,
    )
    .await;
    assert!(init_resp.status().is_success());
    let negotiated = init_resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .expect("initialize response carries MCP-Protocol-Version")
        .to_string();

    // Echo exactly what the server told us, with no Mcp-Method header and
    // no _meta object -- the SEP-2243 machinery that would otherwise
    // demand both must never trigger, because the version we're echoing
    // is < STANDARD_HEADERS (AC10).
    let list_resp = post(
        &http,
        &server.base_url,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {},
        }),
        Some(&negotiated),
    )
    .await;
    let status = list_resp.status();
    let text = list_resp.text().await.expect("read body");
    assert!(
        status.is_success(),
        "tools/list with the echoed negotiated version must succeed: {status} {text}"
    );
    let body: Value = serde_json::from_str(&text).expect("parse tools/list response");
    assert!(
        body.get("error").is_none(),
        "must not be refused with -32020/-32602 or any other JSON-RPC error: {body:?}"
    );
}

#[tokio::test]
async fn ac12_claude_agent_sdk_sequence_lists_signup_with_schema() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // 1. initialize
    let init_resp = post(
        &http,
        &server.base_url,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "claude-agent-sdk", "version": "0.1"}
            }
        }),
        None,
    )
    .await;
    assert!(
        init_resp.status().is_success(),
        "initialize must succeed: {}",
        init_resp.status()
    );
    let negotiated = init_resp
        .headers()
        .get("MCP-Protocol-Version")
        .and_then(|v| v.to_str().ok())
        .expect("initialize response carries MCP-Protocol-Version")
        .to_string();

    // 2. notifications/initialized -- a JSON-RPC notification (no `id`).
    let notify_resp = post(
        &http,
        &server.base_url,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        }),
        Some(&negotiated),
    )
    .await;
    assert!(
        notify_resp.status().is_success(),
        "notifications/initialized must not be refused: {}",
        notify_resp.status()
    );

    // 3. tools/list, carrying the negotiated version, no Mcp-Method, no _meta.
    let list_resp = post(
        &http,
        &server.base_url,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {},
        }),
        Some(&negotiated),
    )
    .await;
    let status = list_resp.status();
    assert!(
        status.is_success(),
        "tools/list must return HTTP 200: {status}"
    );
    let body: Value = list_resp.json().await.expect("parse tools/list response");
    assert!(
        body.get("error").is_none(),
        "tools/list must not be a JSON-RPC error: {body:?}"
    );

    let tools = body["result"]["tools"]
        .as_array()
        .expect("result.tools is an array");
    // PRD-mcphost-session-key requirement 1 widened the anonymous list from
    // `signup` alone to `signup` plus the discoverable `host.*` control
    // plane; PRD-mcphost-publish-first-try requirement 4 added
    // `host.quickstart`, PRD-mcphost-code-tools-warm-pool requirement 3
    // added `host.tool_run`, PRD-grand-loop-billing added the three
    // `billing.*` tools, and PRD-mcphost-rest-bridge P1 requirement added
    // `host.bridge_test`, to that same plane -- eighteen tools total, no
    // client can break on the growth (see that PRD's
    // Migration/compatibility section).
    assert_eq!(
        tools.len(),
        18,
        "signup + host.* (incl. host.quickstart, host.tool_run, host.bridge_test) + \
         host.tool_call + billing.* (3 tools): {tools:?}"
    );
    let tool = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("signup"))
        .expect("signup is in the anonymous tools/list");

    let required = tool["inputSchema"]["required"]
        .as_array()
        .expect("inputSchema.required is an array");
    assert!(
        required.iter().any(|v| v.as_str() == Some("name")),
        "signup's inputSchema must require \"name\": {:?}",
        tool["inputSchema"]
    );
}
