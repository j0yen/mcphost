//! PRD-mcphost-tenant-resource-metadata
//! AC2 (P0) — Given a JWT from I with `aud == "<public>/t/acme/mcp"`, When
//! it calls `host.state.get` on `/t/acme/mcp`, Then the call runs as T with
//! the end-user subject set; When the same JWT calls `/t/other/mcp`
//! (tenant U), Then 401 `wrong_tenant`; When a JWT from I with
//! `aud == "<public>/mcp"` calls `/t/acme/mcp`, Then 401 `wrong_audience`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};

/// A raw `tools/call` so the HTTP status this PRD's extended
/// `oauth_401_upgrade` middleware sets is directly observable, not just
/// the JSON-RPC error body (same shape as
/// `oauthrs_ac03_invalid_tokens_rejected_with_reasons.rs`'s own helper).
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
async fn jwt_runs_as_tenant_on_its_own_path_and_is_scoped_by_tenant_and_audience() {
    let server = TestServer::start().await;
    let (ns_t, key_t) = signup(&server.base_url, "Tenant T").await;
    let (ns_u, _key_u) = signup(&server.base_url, "Tenant U").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key_t).with_path(&format!("/t/{ns_t}/mcp"));

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.tenantprm.example.com";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set with no audience must succeed");

    let tenant_resource = format!("{}/t/{ns_t}/mcp", server.base_url);
    let token = sign(KID_1, priv_pem_1(), issuer, &tenant_resource, "end-user-1", 300);

    // 1. Correct tenant path, aud == the tenant's own canonical URI: runs
    // as T, subject set to the JWT's own sub.
    let bearer_on_t = McpClient::with_bearer(&server.base_url, &token).with_path(&format!("/t/{ns_t}/mcp"));
    let whoami = bearer_on_t
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the tenant's own path must succeed");
    let whoami = common::extract_structured(&whoami);
    assert_eq!(whoami["tenant"], json!(ns_t));
    assert_eq!(whoami["subject"], json!("end-user-1"));

    // 2. Same JWT, called on tenant U's path: wrong_tenant, 401.
    let bearer_on_u = McpClient::with_bearer(&server.base_url, &token).with_path(&format!("/t/{ns_u}/mcp"));
    let resp = bare_call(&bearer_on_u, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse wrong_tenant error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("wrong_tenant"));

    // 3. A JWT with aud == the root resource, called on the tenant path:
    // wrong_audience, 401.
    let root_resource = format!("{}/mcp", server.base_url);
    let root_aud_token = sign(KID_1, priv_pem_1(), issuer, &root_resource, "end-user-2", 300);
    let bearer_root_aud =
        McpClient::with_bearer(&server.base_url, &root_aud_token).with_path(&format!("/t/{ns_t}/mcp"));
    let resp = bare_call(&bearer_root_aud, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = resp.json().await.expect("parse wrong_audience error body");
    assert_eq!(body["error"]["data"]["error_code"], json!("invalid_token"));
    assert_eq!(body["error"]["data"]["error_description"], json!("wrong_audience"));

    std::mem::forget(jwks);
}
