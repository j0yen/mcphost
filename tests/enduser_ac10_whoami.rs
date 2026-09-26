//! PRD-mcphost-end-user-identity
//! AC10 (P0) — Given an OAuth-authenticated call, When
//! `host.enduser.whoami` runs, Then it returns
//! `{subject, issuer, method, verified_at}`; without any end-user
//! identity, it returns null.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;

#[tokio::test]
async fn oauth_caller_gets_identity_no_identity_gets_null() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    // No end-user identity at all -- a plain key-based call.
    let result = key_client
        .tools_call("host.enduser.whoami", json!({}))
        .await
        .expect("whoami must succeed with no identity");
    assert_eq!(extract_structured(&result), serde_json::Value::Null, "{result:?}");

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    let token_u1 = sign(KID_1, priv_pem_1(), issuer, audience, "u1", 300);
    let client_u1 = McpClient::with_bearer(&server.base_url, &token_u1);

    let result = client_u1
        .tools_call("host.enduser.whoami", json!({}))
        .await
        .expect("whoami must succeed for an OAuth caller");
    let structured = extract_structured(&result);
    assert_eq!(structured["subject"], json!("u1"), "{structured}");
    assert_eq!(structured["issuer"], json!(issuer), "{structured}");
    assert_eq!(structured["method"], json!("oauth"), "{structured}");
    assert!(
        structured["verified_at"].as_i64().unwrap_or(0) > 0,
        "verified_at must be set: {structured}"
    );
}
