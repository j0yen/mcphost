//! PRD-mcphost-oauth-discovery-document-contract
//! AC4 (P0) — Given a tenant with two valid issuers, When
//! `host.oauth.issuer_remove` names one, Then the row is gone, the
//! per-tenant and root documents no longer list it, an audit row exists, and
//! the response is `{removed: true}`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn servers(url: String) -> Vec<String> {
    let body: Value = reqwest::get(url).await.expect("GET well-known").json().await.expect("parse JSON");
    body["authorization_servers"]
        .as_array()
        .expect("authorization_servers array")
        .iter()
        .map(|v| v.as_str().expect("string entry").to_string())
        .collect()
}

#[tokio::test]
async fn oauthdoc_ac04_tenant_issuer_remove_updates_documents_and_audits() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Remove Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant row");
    for issuer in ["https://keep.example.com", "https://drop.example.com"] {
        client
            .tools_call(
                "host.oauth.issuer_set",
                json!({"issuer": issuer, "audience": "aud", "jwks_url": "https://idp.example.com/jwks"}),
            )
            .await
            .unwrap_or_else(|e| panic!("register {issuer}: {}", e.message));
    }
    let root_url = format!("{}/.well-known/oauth-protected-resource", server.base_url);
    let tenant_url = format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url);
    for url in [&root_url, &tenant_url] {
        let listed = servers(url.clone()).await;
        assert!(listed.contains(&"https://drop.example.com".to_string()), "precondition: {listed:?}");
    }

    let result = client
        .tools_call("host.oauth.issuer_remove", json!({"issuer": "https://drop.example.com"}))
        .await
        .expect("issuer_remove");
    assert_eq!(result["structuredContent"]["removed"], json!(true), "{result}");

    assert!(
        server.state.db.find_oauth_issuer_by_issuer("https://drop.example.com".to_string()).await.unwrap().is_none(),
        "row must be gone"
    );
    assert!(
        server.state.db.find_oauth_issuer_by_issuer("https://keep.example.com".to_string()).await.unwrap().is_some(),
        "the other issuer must stay"
    );
    for (doc, url) in [("root", &root_url), ("tenant", &tenant_url)] {
        let listed = servers(url.clone()).await;
        assert!(!listed.contains(&"https://drop.example.com".to_string()), "{doc} still lists it: {listed:?}");
        assert!(listed.contains(&"https://keep.example.com".to_string()), "{doc} lost the kept issuer: {listed:?}");
    }

    let audit = server
        .state
        .db
        .list_oauth_audit(tenant.id, None, None, Some("issuer_removed".to_string()), 10, 0)
        .await
        .expect("audit rows");
    assert_eq!(audit.len(), 1, "exactly one issuer_removed audit row: {}", audit.len());
}
