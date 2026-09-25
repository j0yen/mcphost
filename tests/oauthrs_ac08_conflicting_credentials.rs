//! PRD-mcphost-oauth-resource-server
//! AC8 (P1) — Given a request with both a valid key for T1 and a valid
//! bearer for T2, When it runs, Then it is rejected with
//! `conflicting_credentials`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, priv_pem_1, jwk_1, jwks_server, sign};

#[tokio::test]
async fn key_for_t1_and_bearer_for_t2_conflict() {
    let server = TestServer::start().await;
    let (_ns1, key1) = signup(&server.base_url, "T1").await;
    let (ns2, key2) = signup(&server.base_url, "T2").await;

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    McpClient::with_bearer(&server.base_url, &key2)
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("T2 registers issuer I");

    let token_t2 = sign(KID_1, priv_pem_1(), issuer, audience, "user-t2", 300);

    // The Authorization header carries T2's bearer JWT; the tenant_key
    // argument carries T1's raw key -- both valid, different tenants.
    let client = McpClient::with_bearer(&server.base_url, &token_t2);
    let err = client
        .tools_call("host.state.get", json!({"key": "k", "tenant_key": key1}))
        .await
        .expect_err("a valid key for T1 alongside a valid bearer for T2 must be refused");
    assert_eq!(err.error_code.as_deref(), Some("conflicting_credentials"));

    // Same bearer, no tenant_key argument at all: no conflict, runs as T2.
    let ok = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("bearer alone, no tenant_key argument, must still succeed");
    let whoami = common::extract_structured(&ok);
    assert_eq!(whoami["tenant"], json!(ns2));
}
