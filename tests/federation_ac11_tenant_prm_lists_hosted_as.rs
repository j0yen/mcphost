//! PRD-mcphost-federated-end-user-login
//! AC11 (P0) — Given tenant T has an OIDC federation provider registered
//! (`host.oauth.provider_set`), When `GET
//! /.well-known/oauth-protected-resource/t/<ns>/mcp` is called, Then
//! `authorization_servers` lists this host's own hosted-AS issuer (the
//! same value the root document always carries, per
//! PRD-mcphost-hosted-authorization-server) alongside any own BYO issuer
//! the tenant separately registered, hosted AS first and deduplicated --
//! federated end users authenticate through mcphost's own authorization
//! code round trip, not the upstream OIDC provider directly, so a client
//! needs to discover mcphost itself as an AS here, not just the tenant's
//! own issuer list (a live-verifier finding against
//! `protected_resource_metadata_for_tenant`, which previously built
//! `authorization_servers` only from `list_oauth_issuers_by_tenant`). A
//! tenant with no provider registered keeps exactly today's behaviour:
//! only its own issuers, hosted AS absent.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_with_provider_lists_hosted_as_in_tenant_prm() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Federated Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let provider = federation::start().await;
    client
        .tools_call(
            "host.oauth.provider_set",
            json!({
                "issuer": provider.issuer(),
                "client_id": "fed-client",
                "client_secret": "fed-super-secret-value",
            }),
        )
        .await
        .expect("provider_set must succeed");

    let resp = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/{ns}/mcp",
        server.base_url
    ))
    .await
    .expect("GET tenant well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("parse tenant metadata JSON");

    // Hosted AS first, no own `oauth_issuers` row registered for this
    // tenant (only a federation provider), so the list is exactly the
    // host's own URL.
    assert_eq!(
        body["authorization_servers"],
        json!([server.base_url]),
        "a federated tenant's per-tenant document must list this host's own \
         hosted-AS issuer: {body}"
    );

    let own_ns = ns.clone();
    let root_resp = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET root well-known");
    let root_body: serde_json::Value = root_resp.json().await.expect("parse root metadata JSON");
    assert!(
        root_body["authorization_servers"]
            .as_array()
            .expect("authorization_servers must be an array")
            .iter()
            .any(|v| v.as_str() == Some(server.base_url.as_str())),
        "root document must also list the host's own AS for namespace {own_ns}: {root_body}"
    );
}

#[tokio::test]
async fn tenant_with_own_issuer_and_provider_lists_hosted_as_and_own_issuer() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Federated Plus Byo Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let issuer = "https://issuer.byo-plus-federated.example.com";
    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": "https://issuer.byo-plus-federated.example.com/jwks",
            }),
        )
        .await
        .expect("issuer_set must succeed");

    let provider = federation::start().await;
    client
        .tools_call(
            "host.oauth.provider_set",
            json!({
                "issuer": provider.issuer(),
                "client_id": "fed-client-2",
                "client_secret": "fed-super-secret-value-2",
            }),
        )
        .await
        .expect("provider_set must succeed");

    let resp = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/{ns}/mcp",
        server.base_url
    ))
    .await
    .expect("GET tenant well-known");
    let body: serde_json::Value = resp.json().await.expect("parse tenant metadata JSON");

    assert_eq!(
        body["authorization_servers"],
        json!([server.base_url, issuer]),
        "hosted AS must come first, followed by the tenant's own BYO issuer: {body}"
    );
}

#[tokio::test]
async fn tenant_without_provider_keeps_own_issuers_only() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "No Provider Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let issuer = "https://issuer.no-provider.example.com";
    client
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": "https://issuer.no-provider.example.com/jwks",
            }),
        )
        .await
        .expect("issuer_set must succeed");

    let resp = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/{ns}/mcp",
        server.base_url
    ))
    .await
    .expect("GET tenant well-known");
    let body: serde_json::Value = resp.json().await.expect("parse tenant metadata JSON");

    assert_eq!(
        body["authorization_servers"],
        json!([issuer]),
        "a tenant with no federation provider registered must keep today's \
         behaviour unchanged -- only its own issuer, hosted AS absent: {body}"
    );
}
