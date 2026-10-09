//! PRD-mcphost-reachability-alt-host
//! AC2 (P0) — Given two hosts configured, When a request arrives with
//! `Host: alt.example` for `/mcp`, a claim page and `/healthz`, Then each
//! is served exactly as on the primary host with the same tenant key.
//!
//! The server never gates a route on the request's `Host` header at all
//! (`http.rs::build_router_with_session_mode`'s own
//! `disable_allowed_hosts` comment -- the MCP endpoint's security
//! boundary is the bearer key, not Host) so this holds even without
//! `MCPHOST_ALT_PUBLIC_URLS` configured; the test still sets it, matching
//! the AC's own "given two hosts configured" precondition.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn alt_host_header_is_served_identically_to_primary() {
    let server = TestServer::start_with_alts(vec!["https://alt.example".to_string()]).await;

    // /healthz, anonymous body, with a spoofed Host header.
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/healthz", server.base_url))
        .header("Host", "alt.example")
        .send()
        .await
        .expect("GET /healthz with alt Host");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

    // A tenant signs up, then the SAME tenant key authenticates a
    // tools/call sent with the alt Host header over `/mcp`.
    let client = McpClient::new(&server.base_url);
    let raw = client
        .tools_call("signup", json!({"name": "AC2 Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();

    let authed = McpClient::with_bearer(&server.base_url, &key);
    let whoami_raw = authed
        .tools_call_with_header("host.whoami", json!({}), ("Host", "alt.example"))
        .await
        .expect("host.whoami with alt Host");
    let whoami = extract_structured(&whoami_raw);
    assert_eq!(whoami["namespace"], result["namespace"], "same tenant over the alt Host");

    // Claim page: an unknown/expired token still gets the ordinary 410
    // page (not a Host-based 404/refusal) -- proof the route itself is
    // reachable and unaffected by the Host header.
    let resp = http
        .get(format!("{}/claim/not-a-real-token", server.base_url))
        .header("Host", "alt.example")
        .send()
        .await
        .expect("GET /claim/{token} with alt Host");
    assert_eq!(resp.status(), reqwest::StatusCode::GONE);
}
