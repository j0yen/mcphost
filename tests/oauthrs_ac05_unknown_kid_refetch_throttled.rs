//! PRD-mcphost-oauth-resource-server
//! AC5 (P0) — Given an unknown `kid`, When a token arrives, Then the JWKS
//! is refetched once, and a second unknown `kid` within 60 s is rejected
//! without a refetch.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, priv_pem_1, jwk_1, jwks_server, sign};

#[tokio::test]
async fn second_unknown_kid_within_60s_skips_the_refetch() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "OAuth Tenant").await;
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

    // Warm the cache with one real validation first, so the "unknown kid"
    // fetches below aren't confounded with the first-ever "no cache yet"
    // fetch every issuer needs regardless of kid.
    let warm_token = sign(KID_1, priv_pem_1(), issuer, audience, "user-1", 300);
    McpClient::with_bearer(&server.base_url, &warm_token)
        .tools_call("host.whoami", json!({}))
        .await
        .expect("warm-up call with a known kid must succeed");

    let hits_before = jwks.received_requests().await.map(|r| r.len()).unwrap_or(0);

    // First unknown kid: triggers exactly one refetch.
    let unknown_1 = sign("unknown-kid-1", priv_pem_1(), issuer, audience, "user-2", 300);
    let err1 = McpClient::with_bearer(&server.base_url, &unknown_1)
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("an unknown kid must be refused");
    assert_eq!(err1.data.get("error_description"), Some(&json!("bad_signature")));

    let hits_after_first = jwks.received_requests().await.map(|r| r.len()).unwrap_or(0);
    assert_eq!(
        hits_after_first,
        hits_before + 1,
        "the first unknown kid must refetch exactly once"
    );

    // A second, different unknown kid within the 60s cooldown: no refetch.
    let unknown_2 = sign("unknown-kid-2", priv_pem_1(), issuer, audience, "user-3", 300);
    let err2 = McpClient::with_bearer(&server.base_url, &unknown_2)
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a second unknown kid must also be refused");
    assert_eq!(err2.data.get("error_description"), Some(&json!("bad_signature")));

    let hits_after_second = jwks.received_requests().await.map(|r| r.len()).unwrap_or(0);
    assert_eq!(
        hits_after_second, hits_after_first,
        "a second unknown kid within 60s must not trigger another refetch"
    );
}
