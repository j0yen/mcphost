//! PRD-mcphost-federated-end-user-login
//! AC7 (P0) — Given a tenant without a provider, When a client authorizes
//! on its per-tenant resource, Then the key/claim consent form renders as
//! in the hosted AS PRD; Given a tenant with a provider and `owner_login:
//! false`, Then the key form is absent.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, signup};
use serde_json::json;

async fn register_native_client(server: &TestServer) -> String {
    let http = reqwest::Client::new();
    let register: serde_json::Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().unwrap().to_string()
}

fn authorize_url(server: &TestServer, client_id: &str, resource: &str) -> String {
    format!(
        "{}/oauth/authorize?response_type=code&client_id={}&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb&\
         code_challenge=any-challenge-value-at-least-43-characters-long&code_challenge_method=S256&\
         state=xyz&scope=mcp&resource={}",
        server.base_url, client_id, resource
    )
}

#[tokio::test]
async fn tenant_without_provider_renders_key_claim_form() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Acme").await;
    let client_id = register_native_client(&server).await;
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(authorize_url(&server, &client_id, &resource))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "must render the consent page, not redirect");
    let body = resp.text().await.expect("read body");
    assert!(body.contains("tenant_key"), "the key/claim form must be present: {body}");
    assert!(body.contains("claim_code"), "the claim-code field must be present: {body}");
}

#[tokio::test]
async fn tenant_with_provider_owner_login_false_has_no_key_form() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);
    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"}),
        )
        .await
        .expect("provider_set must succeed");

    let client_id = register_native_client(&server).await;
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(authorize_url(&server, &client_id, &resource))
        .send()
        .await
        .expect("GET /oauth/authorize");
    assert!(resp.status().is_redirection(), "must redirect upstream instead of rendering any page: {}", resp.status());
    let body = resp.text().await.unwrap_or_default();
    assert!(!body.contains("tenant_key"), "the key form must be entirely absent: {body}");
}
