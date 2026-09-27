//! PRD-mcphost-tool-scopes-and-consent
//! AC7 (P0) — Given the per-tenant metadata document for T, When read,
//! Then `scopes_supported` is `["mcp","read","write"]` plus T's catalog
//! names; the root document keeps `["mcp"]`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn per_tenant_document_lists_builtins_and_catalog_root_stays_mcp_only() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Metadata Scopes Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("scope_set read");
    client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("scope_set write");
    client
        .tools_call("host.oauth.scope_set", json!({"name": "billing", "description": "Billing ops"}))
        .await
        .expect("scope_set billing");

    let http = reqwest::Client::new();
    let per_tenant: serde_json::Value = http
        .get(format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url))
        .send()
        .await
        .expect("GET per-tenant metadata")
        .json()
        .await
        .expect("parse per-tenant metadata");
    assert_eq!(
        per_tenant["scopes_supported"],
        json!(["mcp", "read", "write", "billing"]),
        "{per_tenant:?}"
    );

    let root: serde_json::Value = http
        .get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .send()
        .await
        .expect("GET root metadata")
        .json()
        .await
        .expect("parse root metadata");
    assert_eq!(root["scopes_supported"], json!(["mcp"]), "root document must be unaffected: {root:?}");
}
