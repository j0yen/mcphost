//! PRD-mcphost-enterprise-managed-auth
//! AC4 (P0) — Given a valid assertion presented with `client_id = "other"`,
//! Then `invalid_client`; Given `scope = "admin"`, Then `invalid_scope`;
//! Given `resource = <public>/t/nope/mcp`, Then `invalid_target`.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
async fn post_token(
    http: &reqwest::Client,
    base_url: &str,
    assertion_jwt: &str,
    client_id: &str,
    resource: &str,
    scope: Option<&str>,
) -> Value {
    let mut form = vec![
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("assertion", assertion_jwt),
        ("client_id", client_id),
        ("resource", resource),
    ];
    if let Some(scope) = scope {
        form.push(("scope", scope));
    }
    http.post(format!("{base_url}/oauth/token"))
        .form(&form)
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response")
}

#[tokio::test]
async fn wrong_client_id_bad_scope_and_unknown_tenant_resource_are_refused() {
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

    let fresh_valid_assertion = || {
        let now = now_unix();
        let claims = json!({
            "iss": issuer, "aud": server.base_url, "sub": "okta|e1",
            "iat": now, "exp": now + 300, "jti": fresh_jti(),
        });
        sign_assertion(KID_1, priv_pem_1(), &claims)
    };

    // client_id = "other" -- not the registered claude-enterprise.
    let resp = post_token(&http, &server.base_url, &fresh_valid_assertion(), "other", &resource, None).await;
    assert_eq!(resp["error"], json!("invalid_client"), "{resp}");

    // scope = "admin" -- not a subset of mcp/catalog.
    let resp =
        post_token(&http, &server.base_url, &fresh_valid_assertion(), "claude-enterprise", &resource, Some("admin"))
            .await;
    assert_eq!(resp["error"], json!("invalid_scope"), "{resp}");

    // resource names an unknown tenant.
    let unknown_resource = format!("{}/t/nope/mcp", server.base_url);
    let resp = post_token(
        &http,
        &server.base_url,
        &fresh_valid_assertion(),
        "claude-enterprise",
        &unknown_resource,
        None,
    )
    .await;
    assert_eq!(resp["error"], json!("invalid_target"), "{resp}");
}
