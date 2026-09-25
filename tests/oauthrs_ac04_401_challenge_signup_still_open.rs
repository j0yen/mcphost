//! PRD-mcphost-oauth-resource-server
//! AC4 (P0) — Given a tool that requires a tenant, When it is called with
//! neither key nor bearer over streamable HTTP, Then the response is 401
//! with a `WWW-Authenticate` header naming the metadata URL; `signup`
//! without credentials still succeeds.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

async fn bare_call(client: &McpClient, name: &str, args: serde_json::Value) -> reqwest::Response {
    client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": args},
            }),
            name,
        )
        .await
}

#[tokio::test]
async fn no_credential_gets_401_with_www_authenticate_naming_the_metadata_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let expected_url = format!("{}/.well-known/oauth-protected-resource", server.base_url);
    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(header.starts_with("Bearer "), "WWW-Authenticate must be a Bearer challenge: {header}");
    assert!(
        header.contains(&format!("resource_metadata=\"{expected_url}\"")),
        "WWW-Authenticate must name the metadata URL: {header}"
    );

    let body: serde_json::Value = resp.json().await.expect("parse error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("tenant_key_missing"));
}

#[tokio::test]
async fn signup_without_credentials_still_succeeds() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = bare_call(&client, "signup", json!({"name": "No Credential Tenant"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("parse signup response");
    assert!(body.get("error").is_none(), "signup must not error: {body:?}");
}
