//! PRD-mcphost-url-bound-tenants
//! AC5 (P0) — Given `GET /u/<secret>/mcp` with a browser `Accept:
//! text/html`, When fetched, Then a short text page explains the URL and
//! how to add it, and the response is not an MCP error.

use crate::common;
use common::{McpClient, extract_structured};
use serde_json::json;

#[tokio::test]
async fn browser_get_sees_an_explainer_not_an_mcp_error() {
    let server = crate::common::TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC5 Tenant"}))
        .await
        .expect("signup");
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();

    let http = reqwest::Client::new();
    let resp = http
        .get(&url)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .expect("GET the URL like a browser");

    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(content_type.starts_with("text/html"), "must be an HTML page, got {content_type}");

    let body = resp.text().await.expect("read body");
    assert!(body.contains(&url), "the page must explain THIS url: {body}");
    assert!(
        body.to_lowercase().contains("mcp server") || body.to_lowercase().contains("add"),
        "the page must explain how to add it: {body}"
    );
    // Not an MCP error: no JSON-RPC error envelope at all.
    assert!(!body.contains("jsonrpc"), "must not be an MCP/JSON-RPC error body: {body}");
    assert!(!body.contains("error_code"), "must not be an MCP/JSON-RPC error body: {body}");
}

#[tokio::test]
async fn an_mcp_client_get_on_the_same_url_is_unaffected() {
    let server = crate::common::TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC5 Tenant Two"}))
        .await
        .expect("signup");
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();

    // An MCP-shaped Accept header must NOT be intercepted by the explainer.
    // Short client timeout: a GET that reached the streamable-HTTP service
    // itself (rather than this test's own explainer-interception check)
    // could otherwise hold the connection open for an SSE stream.
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("client");
    let resp = http
        .get(&url)
        .header("Accept", "application/json, text/event-stream")
        .send()
        .await
        .expect("GET the URL with an MCP Accept header");
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !content_type.starts_with("text/html"),
        "an MCP-shaped GET must not be served the browser explainer, got {content_type}"
    );
}
