//! PRD-mcphost-tenant-resource-metadata
//! AC4 (P0) — Given no credential, When `POST /t/acme/mcp` runs
//! `tools/call host.state.get`, Then 401 with `WWW-Authenticate` naming
//! `/.well-known/oauth-protected-resource/t/acme/mcp` and `scope="mcp"`;
//! the same on `/mcp` names the root document and `scope="mcp"`.

use crate::common;
use common::{McpClient, TestServer, signup};
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

fn www_authenticate(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn no_credential_on_tenant_path_names_the_tenant_metadata_url() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Acme Tenant").await;
    let client = McpClient::new(&server.base_url).with_path(&format!("/t/{ns}/mcp"));

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let expected_url = format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url);
    let header = www_authenticate(&resp);
    assert!(header.starts_with("Bearer "), "WWW-Authenticate must be a Bearer challenge: {header}");
    assert!(
        header.contains(&format!("resource_metadata=\"{expected_url}\"")),
        "WWW-Authenticate must name the tenant's own metadata URL: {header}"
    );
    assert!(header.contains("scope=\"mcp\""), "WWW-Authenticate must carry scope=\"mcp\": {header}");
}

#[tokio::test]
async fn no_credential_on_root_path_names_the_root_metadata_url() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let expected_url = format!("{}/.well-known/oauth-protected-resource", server.base_url);
    let header = www_authenticate(&resp);
    assert!(header.starts_with("Bearer "), "WWW-Authenticate must be a Bearer challenge: {header}");
    assert!(
        header.contains(&format!("resource_metadata=\"{expected_url}\"")),
        "WWW-Authenticate must name the root metadata URL: {header}"
    );
    assert!(header.contains("scope=\"mcp\""), "WWW-Authenticate must carry scope=\"mcp\": {header}");
}
