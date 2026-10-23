//! PRD-mcphost-oauth-metadata-lists-hosted-as
//! AC4 (P0) — Given a stored issuer row whose value is `toolconf-probe`,
//! When the root and the tenant documents are fetched, Then neither lists it
//! and the process log carries one `issuer_url_invalid` warning naming the
//! tenant id and count.

use crate::common;
use common::{TestServer, signup};
use serde_json::{Value, json};

async fn servers(url: String) -> Value {
    let body: Value = reqwest::get(url).await.expect("GET well-known").json().await.expect("parse JSON");
    body["authorization_servers"].clone()
}

#[tokio::test]
async fn stored_non_url_issuer_leaves_both_documents_and_is_warned_about_once() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Legacy Probe Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant row");

    // The setters now refuse this value, so seed the legacy row directly.
    server
        .state
        .db
        .insert_oauth_issuer(
            tenant.id,
            "toolconf-probe".to_string(),
            "aud".to_string(),
            "https://idp.example.com/jwks".to_string(),
            "2026-10-14T00:00:00Z".to_string(),
        )
        .await
        .expect("insert legacy issuer row");

    let buf = common::tracecap::capture_json();

    // What server startup does, once per process.
    let rejected = mcphost::oauth::warn_invalid_stored_issuers(&server.state).await.expect("startup scan");
    assert_eq!(rejected, 1);

    let root = servers(format!("{}/.well-known/oauth-protected-resource", server.base_url)).await;
    let tenant_doc =
        servers(format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url)).await;
    assert_eq!(root, json!([server.base_url]), "root document must not list the stored non-URL issuer");
    assert_eq!(tenant_doc, json!([server.base_url]), "tenant document must not list it either");

    let captured = buf.text();
    let warnings: Vec<Value> = buf
        .json_lines()
        .into_iter()
        .filter(|v| v["fields"]["code"] == json!("issuer_url_invalid"))
        .collect();
    assert_eq!(warnings.len(), 1, "exactly one issuer_url_invalid warning expected: {captured}");
    assert_eq!(warnings[0]["level"], json!("WARN"));
    assert_eq!(warnings[0]["fields"]["tenant_id"], json!(tenant.id));
    assert_eq!(warnings[0]["fields"]["count"], json!(1));
    assert!(!captured.contains("toolconf-probe"), "the stored value must never be logged: {captured}");
}
