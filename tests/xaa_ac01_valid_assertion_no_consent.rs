//! PRD-mcphost-enterprise-managed-auth
//! AC1 (P0) — Given a test identity provider (in-process JWKS) registered
//! by tenant acme as a trusted issuer with `client_id = "claude-enterprise"`,
//! When `POST /oauth/token` presents a valid assertion (`iss` = that
//! provider, `aud` = mcphost AS URL, `sub = okta|e1`, `email`, `exp` in 5
//! min) with `client_id = "claude-enterprise"` and `resource =
//! <public>/t/acme/mcp`, Then an access token returns with `sub` namespaced
//! by the issuer, `aud` equal to the resource, and no consent page or
//! grant approval was involved.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

#[tokio::test]
async fn valid_assertion_mints_namespaced_token_with_no_consent() {
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
    let now = now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": server.base_url,
        "sub": "okta|e1",
        "email": "e1@acme.test",
        "iat": now,
        "exp": now + 300,
        "jti": fresh_jti(),
    });
    let assertion_jwt = sign_assertion(KID_1, priv_pem_1(), &claims);

    // No `/oauth/authorize` redirect, no consent form POST -- this is the
    // one and only HTTP call this flow ever makes.
    let http = reqwest::Client::new();
    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion_jwt.as_str()),
            ("client_id", "claude-enterprise"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");

    assert_eq!(token_resp["token_type"], json!("Bearer"), "unexpected token response: {token_resp}");
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();

    let payload_b64 = access_token.split('.').nth(1).expect("JWT has a payload segment");
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).expect("base64url decode payload");
    let token_claims: Value = serde_json::from_slice(&payload_bytes).expect("parse JWT claims");
    assert_eq!(token_claims["sub"], json!(format!("{issuer}#okta|e1")), "sub must be namespaced by the issuer");
    assert_eq!(token_claims["aud"], json!(resource), "aud must equal the requested per-tenant resource");
}
