//! PRD-mcphost-tenant-resource-metadata
//! AC1 (P0) — Given tenant T with namespace `acme` and issuer I registered
//! with no audience, When `GET /.well-known/oauth-protected-resource/t/acme/mcp`
//! is called, Then it returns 200 with `resource == "<public>/t/acme/mcp"`,
//! `authorization_servers == [I]`, `scopes_supported == ["mcp"]`, and the
//! root document's `authorization_servers` still lists this tenant's issuer
//! (plus, per PRD-mcphost-hosted-authorization-server, the host's own AS --
//! isolation means no OTHER tenant's issuer ever appears there).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn tenant_metadata_document_lists_only_this_tenants_issuer() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let issuer = "https://issuer.acme.example.com";
    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": "https://issuer.acme.example.com/jwks",
            }),
        )
        .await
        .expect("issuer_set with no audience must succeed");

    let resp = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/{ns}/mcp",
        server.base_url
    ))
    .await
    .expect("GET tenant well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.expect("parse tenant metadata JSON");

    assert_eq!(body["resource"], json!(format!("{}/t/{ns}/mcp", server.base_url)));
    assert_eq!(body["authorization_servers"], json!([issuer]));
    assert_eq!(body["scopes_supported"], json!(["mcp"]));
    assert_eq!(body["bearer_methods_supported"], json!(["header"]));

    let root_resp = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET root well-known");
    let root_body: Value = root_resp.json().await.expect("parse root metadata JSON");
    // PRD-mcphost-hosted-authorization-server AC1 (landed ahead of this branch)
    // makes mcphost always its own authorization server, so the root union now
    // carries the host's own issuer URL alongside every tenant's BYO issuer.
    // Tenant isolation -- the property this AC actually tests -- means this
    // tenant's issuer is present and no OTHER tenant's issuer ever appears;
    // it does not mean the host-level AS entry is excluded.
    let root_servers = root_body["authorization_servers"]
        .as_array()
        .expect("authorization_servers must be an array")
        .iter()
        .map(|v| v.as_str().expect("issuer entries must be strings").to_string())
        .collect::<Vec<_>>();
    assert!(
        root_servers.contains(&issuer.to_string()),
        "root document must list this tenant's issuer: {root_body}"
    );
    let own = server.base_url.clone();
    for entry in &root_servers {
        assert!(
            entry == &issuer.to_string() || entry == &own,
            "root document must list only this tenant's issuer and the host's own AS, never another tenant's: {root_body}"
        );
    }
    assert_eq!(
        root_servers.len(),
        2,
        "root document must list exactly this tenant's issuer plus the host's own AS entry: {root_body}"
    );
}

#[tokio::test]
async fn unknown_namespace_metadata_is_404() {
    let server = TestServer::start().await;
    let resp = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/nope/mcp",
        server.base_url
    ))
    .await
    .expect("GET unknown tenant well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
