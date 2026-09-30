//! PRD-mcphost-session-bound-tenant-after-signup
//! AC3 (P0) — Given a fresh session with no `signup`, no `tenant_key` and
//! no header, When it calls `host.catalog.search`, Then the response is
//! HTTP 401 with a `WWW-Authenticate` header, JSON-RPC `-32602`,
//! `data.error_code` `tenant_key_missing`, byte-for-byte the v0.60.35 body
//! shape.
//!
//! Goal 2 / non-goal: this PRD adds a session-bound fallback (see
//! `sessbind_ac01`/`sessbind_ac04`-`ac09`) but must never change what a
//! session that never signed up sees -- this is the regression pin for
//! that guarantee, driven over the same real streamable-HTTP server every
//! other test in this suite uses.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn no_signup_no_key_no_header_is_refused_exactly_as_before() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "host.catalog.search", "arguments": {}},
            }),
            "host.catalog.search",
        )
        .await;

    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let www_authenticate = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .expect("WWW-Authenticate header present")
        .to_string();
    assert!(www_authenticate.starts_with("Bearer "), "must be a Bearer challenge: {www_authenticate}");

    let body: serde_json::Value = resp.json().await.expect("parse error body");
    let error = &body["error"];
    assert_eq!(error["code"], json!(-32602), "JSON-RPC code must be INVALID_PARAMS: {body:?}");
    let data = &error["data"];
    assert_eq!(data["error_code"], json!("tenant_key_missing"), "{body:?}");
    assert_eq!(data["field"], json!("tenant_key"), "{body:?}");
    assert_eq!(
        data["expected"],
        json!("the string signup returned; required only when this connection carries no \
            Authorization: Bearer header"),
        "{body:?}"
    );
    assert_eq!(
        data["example"],
        json!("tk_example_REPLACE_WITH_THE_KEY_SIGNUP_RETURNED"),
        "{body:?}"
    );
    assert_eq!(data["docs"], json!("host.quickstart"), "{body:?}");
}
