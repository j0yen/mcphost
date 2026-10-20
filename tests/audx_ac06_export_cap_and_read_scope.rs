//! PRD-mcphost-audit-export-tamper-evident
//! AC6 (P0) — Given 50 001 rows in the window, When `host.audit.export` runs,
//! Then `export_too_large {rows: 50001}` and nothing is written; Given a
//! read-only OAuth scope, Then the export succeeds (read tool).

use crate::common::{self, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn audx_ac06_over_the_cap_is_export_too_large_and_writes_nothing() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC6 cap").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    server.state.db.insert_oauth_audit_rows_bulk_for_test(tenant.id, 50_001, 0).await.expect("seed 50 001 rows");

    let err = client.tools_call("host.audit.export", json!({})).await.expect_err("over the cap");
    assert_eq!(err.error_code.as_deref(), Some("export_too_large"), "{err:?}");
    assert_eq!(err.data["rows"], json!(50_001), "{err:?}");
    assert!(err.data.get("jsonl").is_none(), "no export is built: {err:?}");
    assert_eq!(server.state.db.audit_exports_count_for_test(tenant.id).await.expect("count"), 0, "nothing is written");

    // A narrower caller-chosen limit refuses the same way; one row under the cap exports.
    let small_err = client.tools_call("host.audit.export", json!({"limit": 10})).await.expect_err("over the limit");
    assert_eq!(small_err.data["rows"], json!(50_001), "{small_err:?}");
}

#[tokio::test]
async fn audx_ac06_a_read_only_oauth_scope_may_export() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC6 scope").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("find").expect("tenant");
    server.state.db.record_tenant_audit(tenant.id, "bob".into(), "key_rotate".into(), None).await.expect("row");

    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, &ns);
    let (_client_id, token) =
        common::oauth_consent_and_token_scoped(&server.base_url, &resource, "tools:read", &[("tenant_key", key.as_str())]).await;
    let bearer = token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}")).to_string();
    let reader = McpClient::with_bearer(&server.base_url, &bearer).with_path(&format!("/t/{ns}/mcp"));

    let out = extract_structured(&reader.tools_call("host.audit.export", json!({})).await.expect("read scope may export"));
    // The seeded row plus the oauth_audit rows the consent flow itself wrote.
    assert!(out["rows"].as_i64().is_some_and(|n| n >= 1), "{out}");
    assert!(out["jsonl"].as_str().is_some_and(|t| t.contains("\"action\":\"key_rotate\"")), "{out}");
    assert_eq!(mcphost::scopes::verb_scope("host.audit.export"), mcphost::scopes::Scope::ToolsRead);
}
