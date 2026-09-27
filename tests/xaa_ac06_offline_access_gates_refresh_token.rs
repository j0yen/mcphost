//! PRD-mcphost-enterprise-managed-auth
//! AC6 (P0) — Given an assertion without the refresh permission, When
//! exchanged, Then no refresh token is returned; Given one with it, Then a
//! rotating refresh token is returned and works once.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use serde_json::{Value, json};

async fn post_token(http: &reqwest::Client, base_url: &str, form: &[(&str, &str)]) -> Value {
    http.post(format!("{base_url}/oauth/token"))
        .form(form)
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response")
}

#[tokio::test]
async fn offline_access_claim_gates_a_once_only_rotating_refresh_token() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://idp.acme-test.example.com";
    key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": format!("{}/jwks", jwks.uri()),
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect("trusted_issuer_set must succeed");

    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let http = reqwest::Client::new();

    // Without offline_access: no refresh_token in the response.
    let now = now_unix();
    let no_refresh_claims = json!({
        "iss": issuer, "aud": server.base_url, "sub": "okta|norefresh",
        "iat": now, "exp": now + 300, "jti": fresh_jti(),
    });
    let no_refresh_assertion = sign_assertion(KID_1, priv_pem_1(), &no_refresh_claims);
    let resp = post_token(
        &http,
        &server.base_url,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &no_refresh_assertion),
            ("client_id", "claude-enterprise"),
            ("resource", &resource),
        ],
    )
    .await;
    assert!(resp["access_token"].as_str().is_some(), "must still mint an access token: {resp}");
    assert!(resp.get("refresh_token").is_none(), "no offline_access claim must mean no refresh_token: {resp}");

    // With offline_access: true, a rotating refresh token that works once.
    let refresh_claims = json!({
        "iss": issuer, "aud": server.base_url, "sub": "okta|withrefresh",
        "iat": now, "exp": now + 300, "jti": fresh_jti(), "offline_access": true,
    });
    let refresh_assertion = sign_assertion(KID_1, priv_pem_1(), &refresh_claims);
    let resp = post_token(
        &http,
        &server.base_url,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &refresh_assertion),
            ("client_id", "claude-enterprise"),
            ("resource", &resource),
        ],
    )
    .await;
    assert!(resp["access_token"].as_str().is_some(), "must mint an access token: {resp}");
    let refresh_token = resp["refresh_token"].as_str().expect("offline_access must yield a refresh_token").to_string();

    // First redemption succeeds and rotates to a new refresh token.
    let redeemed = post_token(
        &http,
        &server.base_url,
        &[("grant_type", "refresh_token"), ("refresh_token", &refresh_token)],
    )
    .await;
    assert!(redeemed["access_token"].as_str().is_some(), "first redemption must mint a fresh access token: {redeemed}");
    let rotated_refresh_token =
        redeemed["refresh_token"].as_str().expect("redemption must rotate to a new refresh_token").to_string();
    assert_ne!(rotated_refresh_token, refresh_token, "rotation must issue a distinct refresh_token");

    // Reusing the already-rotated refresh token fails.
    let reused = post_token(
        &http,
        &server.base_url,
        &[("grant_type", "refresh_token"), ("refresh_token", &refresh_token)],
    )
    .await;
    assert_eq!(reused["error"], json!("invalid_grant"), "reusing a rotated refresh_token must fail: {reused}");
}
