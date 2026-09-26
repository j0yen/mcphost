//! PRD-mcphost-shared-tool-caller-usage
//! AC9 (P0) — Given `admin.usage.top {window: "1d", by: "tool"}`, When
//! called, Then the heaviest tools return and `admin_audit` records the
//! call.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn admin_usage_top_lists_heaviest_tool_and_is_audited() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Heavy Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hot_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish hot_tool");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db query")
        .expect("tenant exists");
    for _ in 0..5 {
        server
            .state
            .db
            .record_call(tenant.id, "hot_tool".to_string(), 5, true, None, None, None, "ok", "external".to_string(), None)
            .await
            .expect("seed call");
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.usage.top", json!({"window": "1d", "by": "tool"}))
        .await
        .expect("admin.usage.top");
    let body = extract_structured(&result);
    let rows = body["rows"].as_array().expect("rows array");
    let expected_key = format!("{ns}.hot_tool");
    let row = rows
        .iter()
        .find(|r| r["key"] == json!(expected_key))
        .unwrap_or_else(|| panic!("expected key '{expected_key}' in rows: {rows:?}"));
    assert_eq!(row["calls"], json!(5), "{row}");

    let audit = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log");
    let audit = common::extract_structured(&audit);
    let entries = audit["entries"].as_array().expect("audit entries array");
    assert!(
        entries.iter().any(|e| e["action"] == json!("usage_top")),
        "admin.usage.top must append a usage_top admin_audit row: {entries:?}"
    );
}
