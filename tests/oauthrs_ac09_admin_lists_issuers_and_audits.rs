//! PRD-mcphost-oauth-resource-server
//! AC9 (P1) — Given two issuers, When `admin.oauth.issuers` runs, Then
//! both appear with tenant id, JWKS age, and per-reason rejection
//! counters, and `admin_audit` records the call.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn admin_issuers_lists_both_and_is_audited() {
    let server = TestServer::start().await;
    let (ns1, key1) = signup(&server.base_url, "T1").await;
    let (ns2, key2) = signup(&server.base_url, "T2").await;

    McpClient::with_bearer(&server.base_url, &key1)
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://issuer-1.example.com", "audience": "aud", "jwks_url": "http://127.0.0.1:1/jwks"}),
        )
        .await
        .expect("T1 registers issuer 1");
    McpClient::with_bearer(&server.base_url, &key2)
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": "https://issuer-2.example.com", "audience": "aud", "jwks_url": "http://127.0.0.1:1/jwks"}),
        )
        .await
        .expect("T2 registers issuer 2");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.oauth.issuers", json!({}))
        .await
        .expect("admin.oauth.issuers must succeed");
    let listed = common::extract_structured(&result);
    let issuers = listed["issuers"].as_array().expect("issuers array");
    assert_eq!(issuers.len(), 2, "both registered issuers must appear: {issuers:?}");

    for (issuer_url, tenant_ns) in [
        ("https://issuer-1.example.com", ns1.as_str()),
        ("https://issuer-2.example.com", ns2.as_str()),
    ] {
        let row = issuers
            .iter()
            .find(|r| r["issuer"] == json!(issuer_url))
            .unwrap_or_else(|| panic!("issuer {issuer_url} must be listed: {issuers:?}"));
        assert_eq!(row["tenant"], json!(tenant_ns), "row must name the owning tenant: {row}");
        assert!(row.get("jwks_age_s").is_some(), "row must carry a jwks_age_s field: {row}");
        assert!(row.get("rejections").is_some(), "row must carry a rejections object: {row}");
    }

    // admin_audit must record this call (unlike admin.tenants/admin.ban.list,
    // which are never audited -- see admin::admin_audit_entry).
    let audit = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log must succeed");
    let audit = common::extract_structured(&audit);
    let entries = audit["entries"].as_array().expect("audit entries array");
    assert!(
        entries.iter().any(|e| e["action"] == json!("oauth_issuers_list")),
        "admin.oauth.issuers must append an oauth_issuers_list admin_audit row: {entries:?}"
    );
}
