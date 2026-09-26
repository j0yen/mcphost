//! PRD-mcphost-end-user-identity
//! AC8 (P0) — Given a call carrying its own end-user identity (u1, via
//! OAuth), When it passes `end_user:"u9"` explicitly to
//! `host.state.get`/`set`, Then the call is rejected -- an explicit
//! subject is only allowed when the call's own identity is absent.

use crate::common;
use common::{McpClient, TestServer, signup};
use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;

#[tokio::test]
async fn explicit_subject_is_rejected_when_the_call_already_carries_an_identity() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

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

    let err = client_u1
        .tools_call("host.state.get", json!({"key": "k", "end_user": "u9"}))
        .await
        .expect_err("an explicit subject must be rejected when the call carries its own identity");
    assert_eq!(err.error_code.as_deref(), Some("end_user_explicit_forbidden"), "{err:?}");

    let err = client_u1
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"who": "u9"}, "end_user": "u9"}),
        )
        .await
        .expect_err("same rejection for host.state.set");
    assert_eq!(err.error_code.as_deref(), Some("end_user_explicit_forbidden"), "{err:?}");
}
