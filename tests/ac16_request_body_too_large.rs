//! AC16 — Given a request body of 2 MiB, When posted to `/mcp`, Then the
//! server answers HTTP 413 without reading the whole body into memory.
//!
//! The "without reading the whole body into memory" half of this AC is a
//! property of `rmcp`'s `StreamableHttpServerConfig::max_request_body_bytes`
//! enforcement (`expect_json`, which this crate configures to the PRD's
//! 1 MiB limit) rather than anything this crate's own code does; what's
//! testable end-to-end here is the observable HTTP-level behavior: an
//! oversized body is rejected with 413.

mod common;
use common::TestServer;

#[tokio::test]
async fn oversized_body_is_rejected_with_413() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let padding = "x".repeat(2 * 1024 * 1024);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"padding": padding},
    });

    let resp = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .expect("send oversized request");

    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
}
