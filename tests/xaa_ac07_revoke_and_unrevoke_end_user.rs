//! PRD-mcphost-enterprise-managed-auth
//! AC7 (P0) — Given e1's grant, When acme calls `host.enduser.revoke`, Then
//! e1's token fails within 60 s and a fresh assertion for e1 is refused
//! with `invalid_grant` `subject_revoked` until acme calls
//! `host.enduser.unrevoke`.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use serde_json::{Value, json};

async fn mint_assertion_token(http: &reqwest::Client, base_url: &str, issuer: &str, resource: &str) -> Value {
    let now = now_unix();
    let claims = json!({
        "iss": issuer, "aud": base_url, "sub": "okta|e1",
        "email": "e1@acme.test", "iat": now, "exp": now + 300, "jti": fresh_jti(),
    });
    let assertion_jwt = sign_assertion(KID_1, priv_pem_1(), &claims);
    http.post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion_jwt.as_str()),
            ("client_id", "claude-enterprise"),
            ("resource", resource),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response")
}

#[tokio::test]
async fn revoke_refuses_existing_token_and_fresh_assertions_until_unrevoked() {
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

    let token_resp = mint_assertion_token(&http, &server.base_url, issuer, &resource).await;
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();

    let tenant_base = format!("{}/t/{ns}", server.base_url);
    let bearer_client = McpClient::with_bearer(&tenant_base, &access_token);
    bearer_client
        .tools_call("host.oauth.grants", json!({}))
        .await
        .expect("e1's token must work before revoke");

    let subject = format!("{issuer}#okta|e1");
    key_client
        .tools_call("host.enduser.revoke", json!({"subject": subject}))
        .await
        .expect("host.enduser.revoke must succeed");

    // e1's already-issued token fails immediately (well within 60s).
    let err = bearer_client
        .tools_call("host.oauth.grants", json!({}))
        .await
        .expect_err("e1's token must fail after revoke");
    assert_eq!(err.error_code.as_deref(), Some("end_user_revoked"), "{err:?}");

    // A fresh assertion for the same subject is refused as invalid_grant/subject_revoked.
    let resp = mint_assertion_token(&http, &server.base_url, issuer, &resource).await;
    assert_eq!(resp["error"], json!("invalid_grant"), "{resp}");
    assert_eq!(resp["error_description"], json!("subject_revoked"), "{resp}");

    key_client
        .tools_call("host.enduser.unrevoke", json!({"subject": subject}))
        .await
        .expect("host.enduser.unrevoke must succeed");

    // A fresh assertion for e1 now succeeds again.
    let resp = mint_assertion_token(&http, &server.base_url, issuer, &resource).await;
    assert!(resp["access_token"].as_str().is_some(), "unrevoke must let a fresh assertion mint a token again: {resp}");
}
