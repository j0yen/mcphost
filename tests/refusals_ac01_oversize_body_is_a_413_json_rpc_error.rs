//! PRD-mcphost-refusals-name-their-cause AC1 -- Given the test server and no
//! tenant key, When a `tools/call` of `host.whoami` padded inside
//! `params._meta` to 1,048,577 bytes is POSTed to `/mcp`, Then the reply is
//! HTTP 413, `application/json` and the § 413 body, never 401 or
//! `tenant_key_missing`, and the tenants row count is unchanged; the same
//! shape at exactly 1,048,576 bytes is answered 200 and mints one tenant; a
//! same-size non-JSON body gets the same 413.

use crate::common;

use common::TestServer;
use mcphost::state::MAX_REQUEST_BODY_BYTES;
use serde_json::Value;

/// A keyless `host.whoami` call (the shape that reaches implicit sign-up),
/// padded inside `params._meta` to exactly `total` bytes.
fn padded_whoami(total: usize) -> String {
    let pre = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"host.whoami","arguments":{},"#,
        r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","#,
        r#""io.modelcontextprotocol/clientCapabilities":{},"#,
        r#""io.modelcontextprotocol/clientInfo":{"name":"refusals-ac01","version":"1"},"pad":""#
    );
    let suf = r#""}}}"#;
    let pad = total - pre.len() - suf.len();
    let body = format!("{pre}{}{suf}", "x".repeat(pad));
    assert_eq!(body.len(), total);
    serde_json::from_str::<Value>(&body).expect("the padded body is valid JSON");
    body
}

async fn post(server: &TestServer, body: String) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "host.whoami")
        .body(body)
        .send()
        .await
        .expect("send")
}

async fn tenant_count(server: &TestServer) -> usize {
    server.state.db.list_tenants().await.expect("list tenants").len()
}

async fn assert_413_body(resp: reqwest::Response, content_length: usize) {
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE, "never 401");
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert!(content_type.starts_with("application/json"), "content-type {content_type}");
    let body: Value = resp.json().await.expect("the 413 body is JSON");
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["id"], Value::Null);
    assert_eq!(body["error"]["code"], -32600);
    let data = &body["error"]["data"];
    assert_eq!(data["error_code"], "request_body_too_large", "{body}");
    assert_ne!(data["error_code"], "tenant_key_missing");
    assert_eq!(data["limit_bytes"], MAX_REQUEST_BODY_BYTES);
    assert_eq!(data["content_length"], content_length);
    assert!(data["hint"].as_str().is_some_and(|h| h.contains("host.blob_put")), "{data}");
    assert_eq!(
        body["error"]["message"],
        format!("request body too large: {content_length} bytes, limit {MAX_REQUEST_BODY_BYTES}")
    );
}

#[tokio::test]
async fn oversize_keyless_whoami_is_a_413_json_rpc_error_and_mints_nobody() {
    let server = TestServer::start().await;
    assert_eq!(tenant_count(&server).await, 0);

    let over = MAX_REQUEST_BODY_BYTES + 1;
    assert_413_body(post(&server, padded_whoami(over)).await, over).await;
    assert_eq!(tenant_count(&server).await, 0, "a refused body signs nobody up");

    // Control: the same shape at exactly the limit is not refused and mints one tenant.
    let resp = post(&server, padded_whoami(MAX_REQUEST_BODY_BYTES)).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "{}", resp.text().await.unwrap_or_default());
    assert_eq!(tenant_count(&server).await, 1, "the at-limit shape is what mints");

    // A same-size non-JSON body gets the same 413 (the refusal runs before parsing).
    assert_413_body(post(&server, "x".repeat(over)).await, over).await;
    assert_eq!(tenant_count(&server).await, 1);
}
