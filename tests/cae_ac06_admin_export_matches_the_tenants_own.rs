//! PRD-mcphost-call-audit-export
//! AC6 (P1) — Given the operator calls `admin.calls_export {tenant}`, When
//! it returns, Then the bundle matches the tenant's own export for the same
//! window and one `admin_audit` row names the tenant.

use crate::common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn cae_ac06_admin_export_equals_the_tenant_export_and_is_audited_once() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Incident Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();
    for i in 0..6 {
        server
            .state
            .db
            .insert_call_row_for_test_full(tenant.id, format!("incident_{i}"), None, None, i % 2 == 0, 4, now - 120 + i)
            .await
            .expect("seed row");
    }
    // An absolute window, so both exports read the same rows.
    let window = json!({"since": now - 3_600, "until": now + 3_600});

    let own = extract_structured(&tenant_client.tools_call("host.calls.export", window.clone()).await.expect("own export"));
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let mut args = window.clone();
    args["tenant"] = json!(ns);
    let operator = extract_structured(&admin.tools_call("admin.calls_export", args).await.expect("admin export"));

    assert_eq!(own["rows"], json!(6), "{own}");
    for field in ["csv", "rows", "sha256", "kid", "alg"] {
        assert_eq!(operator[field], own[field], "{field} must match the tenant's own export");
    }
    // Signed with the same key: the operator's signature is a valid one too
    // (ES256 signing is randomized, so the bytes differ; the key does not).
    assert!(operator["signature"].as_str().is_some_and(|s| !s.is_empty()), "{operator}");

    let audit = extract_structured(&admin.tools_call("admin.audit_log", json!({})).await.expect("audit_log"));
    let exports: Vec<_> = audit["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|e| e["action"] == json!("calls_export"))
        .collect();
    assert_eq!(exports.len(), 1, "exactly one admin_audit row per export: {audit}");
    assert_eq!(exports[0]["target"], json!(ns), "the row names the tenant: {audit}");

    let missing = admin
        .tools_call("admin.calls_export", json!({"tenant": "t_nosuchns"}))
        .await
        .expect_err("an unknown tenant is refused");
    assert_eq!(missing.error_code.as_deref(), Some("tool_not_found"), "{missing:?}");
}
