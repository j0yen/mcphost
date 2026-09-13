//! PRD-mcphost-tenant-tables
//! AC7 — Given `admin.tenant_delete` on a tenant with tables, When it
//! completes, Then the tenant's table storage is gone (cascade verified on
//! disk), and no other tenant's tables changed.
//!
//! `src/admin.rs`'s `remove_tenant_tables` (requirement 5) removes the
//! per-tenant SQLite file at `<data_dir>/tables/<tenant_id>.db` -- the same
//! path `tables::table_db_path` derives (see `src/tables.rs` line ~227) --
//! but nothing previously asserted the file is actually gone from disk
//! after the call: `tests/tenant_delete_ac01_cascade_delete.rs` proves the
//! DB-row cascade (tools/secrets/calls/logs) for a different PRD, and
//! nothing in that suite or `src/tables.rs`'s own `#[cfg(test)]` module
//! ever creates a table then deletes the owning tenant.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn table_db_path(data_dir: &std::path::Path, tenant_id: i64) -> std::path::PathBuf {
    data_dir.join("tables").join(format!("{tenant_id}.db"))
}

#[tokio::test]
async fn deleting_a_tenant_removes_its_table_file_and_leaves_others_untouched() {
    let server = TestServer::start().await;
    let data_dir = server.state.db.data_dir().to_path_buf();

    // Two tenants, each with a table -- so the assertion that tenant A's
    // removal leaves tenant B's tables alone is actually exercised, not
    // vacuously true.
    let (ns_a, key_a) = signup(&server.base_url, "Doomed Table Tenant").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.table.create",
            json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("tenant a create table");
    client_a
        .tools_call(
            "host.table.append",
            json!({"table": "metrics", "rows": [{"metric": "cpu", "value": 0.9}]}),
        )
        .await
        .expect("tenant a append row");

    let (ns_b, key_b) = signup(&server.base_url, "Surviving Table Tenant").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    client_b
        .tools_call(
            "host.table.create",
            json!({"name": "metrics", "columns": {"metric": "text", "value": "real"}}),
        )
        .await
        .expect("tenant b create table");

    let tenant_a = server
        .state
        .db
        .find_tenant_by_namespace(ns_a.clone())
        .await
        .unwrap()
        .expect("tenant a row");
    let tenant_b = server
        .state
        .db
        .find_tenant_by_namespace(ns_b.clone())
        .await
        .unwrap()
        .expect("tenant b row");

    let path_a = table_db_path(&data_dir, tenant_a.id);
    let path_b = table_db_path(&data_dir, tenant_b.id);
    assert!(path_a.is_file(), "tenant a's table file must exist before delete: {path_a:?}");
    assert!(path_b.is_file(), "tenant b's table file must exist before delete: {path_b:?}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns_a}))
        .await
        .expect("admin.tenant_delete");
    let structured = extract_structured(&result);
    assert_eq!(structured["tenant"], json!(ns_a));

    // The cascade this AC names: the deleted tenant's table storage is
    // actually gone on disk (not just its DB rows).
    assert!(
        !path_a.exists(),
        "tenant a's table file must be removed from disk after tenant_delete: {path_a:?}"
    );
    // No other tenant's tables changed.
    assert!(path_b.is_file(), "tenant b's table file must survive tenant a's delete: {path_b:?}");
    let b_rows = extract_structured(
        &client_b
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM metrics"}))
            .await
            .expect("tenant b's own table must still be queryable after tenant a's delete"),
    );
    assert!(b_rows["rows"].as_array().expect("rows array").is_empty());
}

/// A tenant that never created a table deletes cleanly -- `remove_tenant_tables`
/// is documented best-effort for exactly this case (no file, nothing to
/// remove); this asserts the delete still succeeds rather than erroring on
/// a missing file.
#[tokio::test]
async fn deleting_a_tenant_with_no_tables_still_succeeds() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Tableless Tenant").await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.tenant_delete", json!({"tenant": ns.clone()}))
        .await
        .expect("admin.tenant_delete must succeed even with no table file to remove");
    let structured = extract_structured(&result);
    assert_eq!(structured["tenant"], json!(ns));
}
