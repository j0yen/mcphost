//! PRD-mcphost-oauth-discovery-document-contract
//! AC5 (P0) — Given an admin and a tenant's invalid issuer, When
//! `admin.oauth.issuer_remove` runs with a reason, Then the row is removed,
//! the audit row carries the reason, and the tenant's own documents rebuild
//! without it.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn oauthdoc_ac05_admin_issuer_remove_clears_invalid_row_with_reason() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Admin Remove Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().expect("tenant row");
    McpClient::with_bearer(&server.base_url, &key)
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://keep.example.com", "audience": "aud", "jwks_url": "https://idp.example.com/jwks"}),
        )
        .await
        .expect("register the valid issuer");
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
        .expect("insert invalid row");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call(
            "admin.oauth.issuer_remove",
            json!({"tenant_id": tenant.id, "issuer": "toolconf-probe", "reason": "probe residue"}),
        )
        .await
        .expect("admin.oauth.issuer_remove");
    assert_eq!(result["structuredContent"]["removed"], json!(true), "{result}");

    assert!(
        server.state.db.find_oauth_issuer_by_issuer("toolconf-probe".to_string()).await.unwrap().is_none(),
        "row must be gone"
    );
    let audit = server
        .state
        .db
        .list_oauth_audit(tenant.id, None, None, Some("issuer_removed".to_string()), 10, 0)
        .await
        .expect("audit rows");
    assert_eq!(audit.len(), 1, "one audit row");
    assert_eq!(audit[0].reason.as_deref(), Some("probe residue"), "audit row carries the reason");

    let tenant_doc: Value = reqwest::get(format!(
        "{}/.well-known/oauth-protected-resource/t/{ns}/mcp",
        server.base_url
    ))
    .await
    .expect("GET tenant document")
    .json()
    .await
    .expect("parse JSON");
    assert_eq!(
        tenant_doc["authorization_servers"],
        json!([server.base_url, "https://keep.example.com"]),
        "tenant document rebuilds with the hosted AS and the kept issuer"
    );

    let doctor = admin
        .tools_call("host.doctor", json!({"tenant": ns}))
        .await
        .expect("admin host.doctor");
    assert_eq!(
        doctor["structuredContent"]["oauth_issuers"],
        json!({"rows": 1, "invalid": []}),
        "doctor sees one valid row left"
    );

    // A reason is required.
    let err = admin
        .tools_call("admin.oauth.issuer_remove", json!({"tenant_id": tenant.id, "issuer": "https://keep.example.com"}))
        .await
        .expect_err("missing reason must be refused");
    assert!(err.message.contains("reason"), "{}", err.message);
}
