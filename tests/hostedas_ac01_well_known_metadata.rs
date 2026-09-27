//! PRD-mcphost-hosted-authorization-server
//! AC1 (P0) — Given a fresh host, When `GET
//! /.well-known/oauth-authorization-server` and `GET
//! /.well-known/openid-configuration` are called, Then both return the
//! endpoints, `code_challenge_methods_supported == ["S256"]`,
//! `client_id_metadata_document_supported == true`, and `GET
//! /.well-known/oauth-protected-resource` lists the host's own URL in
//! `authorization_servers`.

use crate::common;
use common::TestServer;
use serde_json::Value;

async fn fetch(base_url: &str, path: &str) -> Value {
    let resp = reqwest::get(format!("{base_url}{path}")).await.expect("GET well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    resp.json().await.expect("parse well-known JSON")
}

fn assert_authorization_server_metadata(body: &Value, base_url: &str) {
    assert_eq!(body["issuer"], Value::String(base_url.to_string()));
    assert_eq!(
        body["authorization_endpoint"],
        Value::String(format!("{base_url}/oauth/authorize"))
    );
    assert_eq!(body["token_endpoint"], Value::String(format!("{base_url}/oauth/token")));
    assert_eq!(
        body["registration_endpoint"],
        Value::String(format!("{base_url}/oauth/register"))
    );
    assert_eq!(body["revocation_endpoint"], Value::String(format!("{base_url}/oauth/revoke")));
    assert_eq!(
        body["jwks_uri"],
        Value::String(format!("{base_url}/.well-known/jwks.json"))
    );
    assert_eq!(body["response_types_supported"], serde_json::json!(["code"]));
    // PRD-mcphost-enterprise-managed-auth requirement 2 (AC5): the
    // JWT-bearer grant type joins the pre-existing two.
    assert_eq!(
        body["grant_types_supported"],
        serde_json::json!([
            "authorization_code",
            "refresh_token",
            "urn:ietf:params:oauth:grant-type:jwt-bearer"
        ])
    );
    assert_eq!(body["code_challenge_methods_supported"], serde_json::json!(["S256"]));
    assert_eq!(body["client_id_metadata_document_supported"], Value::Bool(true));
}

#[tokio::test]
async fn oauth_authorization_server_and_openid_configuration_and_protected_resource() {
    let server = TestServer::start().await;

    let as_metadata = fetch(&server.base_url, "/.well-known/oauth-authorization-server").await;
    assert_authorization_server_metadata(&as_metadata, &server.base_url);

    let oidc_metadata = fetch(&server.base_url, "/.well-known/openid-configuration").await;
    assert_authorization_server_metadata(&oidc_metadata, &server.base_url);

    let jwks = fetch(&server.base_url, "/.well-known/jwks.json").await;
    let keys = jwks["keys"].as_array().expect("keys array");
    assert_eq!(keys.len(), 1, "exactly one published signing key: {jwks}");
    assert_eq!(keys[0]["kty"], Value::String("EC".to_string()));
    assert_eq!(keys[0]["crv"], Value::String("P-256".to_string()));
    assert!(keys[0]["kid"].as_str().is_some_and(|k| !k.is_empty()));

    let protected_resource = fetch(&server.base_url, "/.well-known/oauth-protected-resource").await;
    let authorization_servers = protected_resource["authorization_servers"]
        .as_array()
        .expect("authorization_servers array");
    assert!(
        authorization_servers.contains(&Value::String(server.base_url.clone())),
        "host's own URL must be listed as an authorization server: {protected_resource}"
    );
}
