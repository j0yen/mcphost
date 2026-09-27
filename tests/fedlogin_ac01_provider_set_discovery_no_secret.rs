//! PRD-mcphost-federated-end-user-login
//! AC1 (P0) — Given a test OIDC provider (in-process: discovery, JWKS,
//! authorize, token issuing an `id_token` for `sub=u1,
//! email=u1@acme.test, name=U One`), When tenant acme calls
//! `host.oauth.provider_set` with its issuer, client id and secret, Then
//! discovery is fetched once and `host.oauth.provider` shows the
//! endpoints with no secret.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn provider_set_fetches_discovery_once_and_provider_hides_secret() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    let client_secret = "acme-super-secret-value";

    let set_result = key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({
                "issuer": provider.issuer(),
                "client_id": "acme-client",
                "client_secret": client_secret,
                "scopes": ["openid", "email", "profile"],
            }),
        )
        .await
        .expect("provider_set must succeed");
    let set_structured = extract_structured(&set_result);

    let discovery_requests = provider
        .server
        .received_requests()
        .await
        .expect("wiremock must record requests")
        .into_iter()
        .filter(|r| r.url.path() == "/.well-known/openid-configuration")
        .count();
    assert_eq!(discovery_requests, 1, "discovery must be fetched exactly once");

    for structured in [&set_structured, &{
        let get_result = key_client.tools_call("host.oauth.provider", json!({})).await.expect("provider must succeed");
        extract_structured(&get_result)
    }] {
        assert_eq!(structured["issuer"], json!(provider.issuer()));
        assert_eq!(structured["client_id"], json!("acme-client"));
        assert_eq!(structured["authorization_endpoint"], json!(provider.authorization_endpoint()));
        assert_eq!(structured["token_endpoint"], json!(provider.token_endpoint()));
        assert_eq!(structured["jwks_uri"], json!(provider.jwks_uri()));
        assert_eq!(structured["require_verified_email"], json!(true));
        assert_eq!(structured["owner_login"], json!(false));

        let dump = structured.to_string();
        assert!(!dump.contains(client_secret), "client_secret must never appear: {dump}");
        assert!(structured.get("client_secret").is_none(), "no client_secret field at all: {structured}");
    }
}
