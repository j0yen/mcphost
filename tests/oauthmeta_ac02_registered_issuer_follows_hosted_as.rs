//! PRD-mcphost-oauth-metadata-lists-hosted-as
//! AC2 (P0) — Given T registered issuer I with `host.oauth.issuer_set`, When
//! the tenant document is fetched, Then `authorization_servers ==
//! ["<public_url>", I]`; Given I equals `public_url`, Then it appears once.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn tenant_servers(base_url: &str, ns: &str) -> Value {
    let body: Value = reqwest::get(format!("{base_url}/.well-known/oauth-protected-resource/t/{ns}/mcp"))
        .await
        .expect("GET tenant well-known")
        .json()
        .await
        .expect("parse tenant metadata JSON");
    body["authorization_servers"].clone()
}

#[tokio::test]
async fn registered_issuer_follows_the_hosted_as() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Issuer Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let issuer = "https://idp.oauthmeta-ac02.example.com";
    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "jwks_url": "https://idp.oauthmeta-ac02.example.com/jwks"}),
        )
        .await
        .expect("issuer_set must succeed");

    assert_eq!(tenant_servers(&server.base_url, &ns).await, json!([server.base_url, issuer]));
}

#[tokio::test]
async fn issuer_equal_to_public_url_is_listed_once() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Self Issuer Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": server.base_url, "jwks_url": format!("{}/jwks", server.base_url)}),
        )
        .await
        .expect("issuer_set of the public URL must succeed");

    assert_eq!(
        tenant_servers(&server.base_url, &ns).await,
        json!([server.base_url]),
        "an issuer equal to public_url must appear exactly once"
    );
}
