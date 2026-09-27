//! PRD-mcphost-tenant-resource-metadata
//! AC5 (P0) — Given a JWT from I with `scope: "profile"`, When it calls a
//! tool on `/t/acme/mcp`, Then 403 with `error="insufficient_scope"`,
//! `scope="mcp"` and `resource_metadata`; Given a JWT with no `scope`
//! claim, Then the call runs.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign, sign_with_scope};

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

async fn setup() -> (TestServer, String, String) {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key).with_path(&format!("/t/{ns}/mcp"));

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.tenantprm-scope.example.com".to_string();
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");
    std::mem::forget(jwks);
    (server, ns, issuer)
}

#[tokio::test]
async fn scope_without_mcp_is_refused_with_insufficient_scope() {
    let (server, ns, issuer) = setup().await;
    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let token = sign_with_scope(KID_1, priv_pem_1(), &issuer, &resource, "user-1", 300, "profile");
    let client = McpClient::with_bearer(&server.base_url, &token).with_path(&format!("/t/{ns}/mcp"));

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);

    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(header.contains("error=\"insufficient_scope\""), "must name insufficient_scope: {header}");
    assert!(header.contains("scope=\"mcp\""), "must name scope=\"mcp\": {header}");
    assert!(header.contains("resource_metadata="), "must carry resource_metadata: {header}");

    let body: serde_json::Value = resp.json().await.expect("parse error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("insufficient_scope"));
}

#[tokio::test]
async fn no_scope_claim_at_all_is_unrestricted() {
    let (server, ns, issuer) = setup().await;
    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    // `sign` (unlike `sign_with_scope`) never emits a `scope` claim.
    let token = sign(KID_1, priv_pem_1(), &issuer, &resource, "user-2", 300);
    let client = McpClient::with_bearer(&server.base_url, &token).with_path(&format!("/t/{ns}/mcp"));

    client
        .tools_call("host.state.set", json!({"key": "greeting", "value": "hi"}))
        .await
        .expect("a token with no scope claim at all must not be refused");
}
