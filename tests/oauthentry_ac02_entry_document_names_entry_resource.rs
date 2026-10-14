//! PRD-mcphost-oauth-first-entry-url
//! AC2 (P0) — Given `GET /.well-known/oauth-protected-resource/login/mcp`,
//! When fetched, Then `resource == "<public_url>/login/mcp"` and
//! `authorization_servers == ["<public_url>"]`.

use crate::common;
use common::TestServer;
use mcphost::oauth::{OAUTH_ENTRY_PATH, entry_metadata_path};
use serde_json::{Value, json};

#[tokio::test]
async fn entry_document_names_the_entry_resource_and_the_hosted_authorization_server() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!("{}{}", server.base_url, entry_metadata_path()))
        .await
        .expect("GET entry document");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let doc: Value = resp.json().await.expect("entry document json");
    assert_eq!(doc["resource"], json!(format!("{}{OAUTH_ENTRY_PATH}", server.base_url)));
    assert_eq!(doc["authorization_servers"], json!([server.base_url.clone()]));
}

#[tokio::test]
async fn entry_document_scopes_and_bearer_methods_match_the_root_document() {
    let server = TestServer::start().await;
    let root: Value = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET root document")
        .json()
        .await
        .expect("root json");
    let entry: Value = reqwest::get(format!("{}{}", server.base_url, entry_metadata_path()))
        .await
        .expect("GET entry document")
        .json()
        .await
        .expect("entry json");
    assert_eq!(entry["scopes_supported"], root["scopes_supported"], "R3: scopes as the root document");
    assert_eq!(entry["bearer_methods_supported"], root["bearer_methods_supported"]);
}

#[tokio::test]
async fn entry_document_names_no_other_authorization_server_even_when_a_tenant_registers_one() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "issuer-owner").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let _ = client
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": "https://idp.example.test",
                "jwks_url": "https://idp.example.test/jwks.json",
                "audience": "x",
            }),
        )
        .await;
    let doc: Value = reqwest::get(format!("{}{}", server.base_url, entry_metadata_path()))
        .await
        .expect("GET entry document")
        .json()
        .await
        .expect("entry json");
    assert_eq!(doc["authorization_servers"], json!([server.base_url.clone()]), "never enumerates tenant issuers");
}
