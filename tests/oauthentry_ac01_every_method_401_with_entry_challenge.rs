//! PRD-mcphost-oauth-first-entry-url
//! AC1 (P0) — Given no credential, When `POST /login/mcp` carries
//! `initialize`, `tools/list`, or `tools/call`, Then each answers 401 with
//! `WWW-Authenticate: Bearer resource_metadata="<public_url>/.well-known/
//! oauth-protected-resource/login/mcp", scope="mcp"` and no tenant is created.

use crate::common;
use common::TestServer;
use mcphost::oauth::OAUTH_ENTRY_PATH;
use serde_json::{Value, json};

fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

#[tokio::test]
async fn every_method_on_the_entry_path_is_401_with_the_entry_challenge_and_creates_no_tenant() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let before = server.state.db.list_tenants().await.expect("list tenants").len();
    let want = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource{OAUTH_ENTRY_PATH}\", scope=\"mcp\"",
        server.base_url
    );

    let calls = [
        rpc(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "probe", "version": "0"}}),
        ),
        rpc("tools/list", json!({})),
        rpc("tools/call", json!({"name": "host.whoami", "arguments": {}})),
    ];
    for body in calls {
        let method = body["method"].as_str().unwrap().to_string();
        let resp = http
            .post(format!("{}{OAUTH_ENTRY_PATH}", server.base_url))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .await
            .expect("POST entry path");
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "{method} must be 401");
        let challenge = resp
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .unwrap_or_else(|| panic!("{method}: WWW-Authenticate header missing"))
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(challenge, want, "{method}: challenge names the entry document");
    }

    let after = server.state.db.list_tenants().await.expect("list tenants").len();
    assert_eq!(before, after, "an unauthenticated entry request must create no tenant");
}

#[tokio::test]
async fn a_get_on_the_entry_path_without_a_bearer_is_also_401() {
    let server = TestServer::start().await;
    let resp = reqwest::Client::new()
        .get(format!("{}{OAUTH_ENTRY_PATH}", server.base_url))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("GET entry path");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert!(resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_some());
}
