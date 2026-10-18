//! PRD-mcphost-oauth-discovery-document-contract
//! AC2 (P0) — Given an `oauth_issuers` row with issuer `toolconf-probe`
//! inserted directly into the test database, When the root and per-tenant
//! documents are built, Then neither lists it and `host.doctor` reports one
//! invalid row with that id and reason `not_https_url`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn servers(url: String) -> Value {
    let body: Value = reqwest::get(url).await.expect("GET well-known").json().await.expect("parse JSON");
    body["authorization_servers"].clone()
}

#[tokio::test]
async fn oauthdoc_ac02_non_url_row_leaves_documents_and_doctor_reports_it() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Doctor Probe Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant row");
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
    let row_id = server
        .state
        .db
        .find_oauth_issuer_by_issuer("toolconf-probe".to_string())
        .await
        .unwrap()
        .expect("stored row")
        .id;

    let root = servers(format!("{}/.well-known/oauth-protected-resource", server.base_url)).await;
    let tenant_doc =
        servers(format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url)).await;
    assert_eq!(root, json!([server.base_url]), "root document must not list toolconf-probe");
    assert_eq!(tenant_doc, json!([server.base_url]), "tenant document must not list toolconf-probe");

    let expected = json!({"rows": 1, "invalid": [{"id": row_id, "tenant_id": tenant.id, "reason": "not_https_url"}]});
    let doctor = McpClient::with_bearer(&server.base_url, &key)
        .tools_call("host.doctor", json!({}))
        .await
        .expect("tenant host.doctor");
    assert_eq!(doctor["structuredContent"]["oauth_issuers"], expected, "tenant doctor: {doctor}");

    let admin_doctor = McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("host.doctor", json!({"tenant": ns}))
        .await
        .expect("admin host.doctor");
    assert_eq!(admin_doctor["structuredContent"]["oauth_issuers"], expected, "admin doctor: {admin_doctor}");
}
