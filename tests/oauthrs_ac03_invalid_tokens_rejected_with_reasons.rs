//! PRD-mcphost-oauth-resource-server
//! AC3 (P0) — Given the same setup as AC2, When a JWT with a wrong
//! signature, an expired `exp`, or `aud≠A` is presented, Then each returns
//! 401 `invalid_token` with the matching `error_description` and the
//! rejection counter for that reason increments.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, priv_pem_1, priv_pem_2, jwk_1, jwks_server, sign};

/// A raw `tools/call` with no `_meta` (same shape
/// `attribdefault_ac1_bare_signup_client_info_is_null.rs`'s own
/// `bare_tools_call` helper uses) so the HTTP status this PRD's
/// `oauth_401_upgrade` middleware sets is directly observable, not just
/// the JSON-RPC error body `McpClient::tools_call` alone would parse.
async fn bare_call(client: &McpClient, name: &str, args: serde_json::Value) -> reqwest::Response {
    client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": args},
            }),
            name,
        )
        .await
}

async fn setup() -> (TestServer, String, String) {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "OAuth Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com".to_string();
    let audience = "mcphost-test-audience".to_string();
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    // jwks_server must outlive the calls below.
    std::mem::forget(jwks);
    (server, issuer, audience)
}

async fn rejection_count(server: &TestServer, issuer: &str, reason: &str) -> i64 {
    server
        .state
        .db
        .oauth_rejection_counts(issuer.to_string())
        .await
        .expect("read rejection counts")
        .into_iter()
        .find(|(r, _)| r == reason)
        .map(|(_, n)| n)
        .unwrap_or(0)
}

#[tokio::test]
async fn wrong_signature_is_rejected_and_counted() {
    let (server, issuer, audience) = setup().await;
    let before = rejection_count(&server, &issuer, "bad_signature").await;

    // Header names the real, published kid, but the bytes are signed with
    // an unrelated private key -- signature verification must fail.
    let token = sign(KID_1, priv_pem_2(), &issuer, &audience, "user-1", 300);
    let client = McpClient::with_bearer(&server.base_url, &token);
    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("invalid_token"));
    assert_eq!(body["error"]["data"]["error_description"], json!("bad_signature"));

    let after = rejection_count(&server, &issuer, "bad_signature").await;
    assert_eq!(after, before + 1, "bad_signature counter must increment");
}

#[tokio::test]
async fn expired_token_is_rejected_and_counted() {
    let (server, issuer, audience) = setup().await;
    let before = rejection_count(&server, &issuer, "expired").await;

    // 120s in the past, well past the 60s skew allowance.
    let token = sign(KID_1, priv_pem_1(), &issuer, &audience, "user-1", -120);
    let client = McpClient::with_bearer(&server.base_url, &token);
    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("invalid_token"));
    assert_eq!(body["error"]["data"]["error_description"], json!("expired"));

    let after = rejection_count(&server, &issuer, "expired").await;
    assert_eq!(after, before + 1, "expired counter must increment");
}

#[tokio::test]
async fn wrong_audience_is_rejected_and_counted() {
    let (server, issuer, audience) = setup().await;
    let before = rejection_count(&server, &issuer, "wrong_audience").await;
    let _ = &audience;

    let token = sign(KID_1, priv_pem_1(), &issuer, "not-the-registered-audience", "user-1", 300);
    let client = McpClient::with_bearer(&server.base_url, &token);
    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("invalid_token"));
    assert_eq!(body["error"]["data"]["error_description"], json!("wrong_audience"));

    let after = rejection_count(&server, &issuer, "wrong_audience").await;
    assert_eq!(after, before + 1, "wrong_audience counter must increment");
}
