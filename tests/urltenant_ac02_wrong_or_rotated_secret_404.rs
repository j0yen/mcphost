//! PRD-mcphost-url-bound-tenants
//! AC2 (P0) — Given a random or rotated secret, When any request hits
//! `/u/<wrong>/mcp`, Then HTTP 404 with the standard error body, no
//! `WWW-Authenticate` header, and no resource-metadata link.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

async fn assert_standard_404(resp: reqwest::Response) {
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    assert!(
        resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_none(),
        "a 404 for an unknown URL secret must carry no WWW-Authenticate header"
    );
    let body = resp.bytes().await.expect("read body");
    assert!(
        body.is_empty(),
        "the standard 404 error body (axum's own empty fallback) must be empty: {body:?}"
    );
}

#[tokio::test]
async fn a_secret_that_was_never_issued_404s() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(format!("{}/u/0000000000000000000000000A/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST to an unissued secret");
    assert_standard_404(resp).await;

    // No credential at all hits the same unknown secret -- same 404, never
    // a different shape based on what else the request carried.
    let resp = http
        .get(format!("{}/u/0000000000000000000000000A/mcp", server.base_url))
        .header("Accept", "application/json, text/event-stream")
        .send()
        .await
        .expect("GET an unissued secret");
    assert_standard_404(resp).await;
}

#[tokio::test]
async fn a_rotated_away_secret_404s_on_the_very_next_request() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC2 Tenant"}))
        .await
        .expect("signup");
    let rotated = session
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("host.key_rotate mints a first URL");
    let old_url = extract_structured(&rotated)["url"]
        .as_str()
        .expect("key_rotate returns a url")
        .to_string();
    let old_path = old_url.trim_start_matches(&server.base_url).to_string();

    // The old secret works once, before rotation.
    let old_client = McpClient::new(&server.base_url).with_path(&old_path);
    old_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the freshly minted URL must work before any rotation");

    // Rotate again -- the secret above is now dead.
    session
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("second host.key_rotate");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}{old_path}", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .expect("POST to the now-rotated-away secret");
    assert_standard_404(resp).await;
}
