//! PRD-mcphost-oauth-resource-server
//! AC6 (P0) — Given the JWKS server is down, When a token arrives for an
//! issuer with no cached JWKS, Then the token is rejected with
//! `unknown_issuer` and the host stays healthy.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{DOWN_JWKS_URL, priv_pem_1, sign};

#[tokio::test]
async fn unreachable_jwks_rejects_unknown_issuer_and_host_stays_healthy() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "OAuth Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": DOWN_JWKS_URL}),
        )
        .await
        .expect("issuer_set must succeed even though jwks_url is unreachable");

    let token = sign("some-kid", priv_pem_1(), issuer, audience, "user-1", 300);
    let err = McpClient::with_bearer(&server.base_url, &token)
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a token for an issuer with no reachable JWKS must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invalid_token"));
    assert_eq!(err.data.get("error_description"), Some(&json!("unknown_issuer")));

    // The host itself must stay healthy (admin bearer sees db_ok: true).
    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse healthz");
    assert_eq!(health["db_ok"], json!(true));
}
