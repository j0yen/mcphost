//! PRD-mcphost-provenance-audit
//! AC5 — Given an admin-bearer tenant deletion, When it executes, Then an
//! `admin_audit` row exists with actor key-id, action, target, and
//! timestamp, and the audit endpoint returns it.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_delete_through_the_real_mcp_surface_leaves_an_audit_row() {
    let server = TestServer::start().await;
    let (tenant_ns, _key) = signup(&server.base_url, "Audited For Deletion").await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Drive the mutation through the real MCP/HTTP surface -- the
    // audit-writing logic lives one layer up in
    // `handler::dispatch_admin_tool`, not in `admin::tenant_delete`
    // itself, so calling `admin::tenant_delete` directly wouldn't exercise
    // it.
    admin
        .tools_call("admin.tenant_delete", json!({"tenant": tenant_ns.clone()}))
        .await
        .expect("admin.tenant_delete");

    let result = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .expect("admin.audit_log");
    let structured = extract_structured(&result);
    let entries = structured["entries"]
        .as_array()
        .expect("entries array")
        .clone();
    assert!(!entries.is_empty(), "{structured:?}");

    let row = entries
        .iter()
        .find(|e| e["action"] == json!("tenant_delete") && e["target"] == json!(tenant_ns))
        .unwrap_or_else(|| panic!("no tenant_delete row for {tenant_ns} in {entries:?}"));

    assert!(
        row["actor_key_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{row:?}"
    );
    assert!(row["created_unix"].as_i64().is_some(), "{row:?}");
}
