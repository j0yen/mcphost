//! PRD-mcphost-implicit-signup-mints-only-on-success
//! AC3 (P0) — Given a keyless first call that passes precheck and then fails,
//! times out, or panics in dispatch (test fault hooks), When it returns, Then
//! the caller gets the handler's error plus `data.get_a_key` naming `signup`
//! and `host.quickstart`, every table with a `tenant_id` column or a
//! `PRAGMA foreign_key_list` entry referencing `tenants` has zero rows for the
//! minted id, and one `implicit_signup_rolled_back` admin_events row exists.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::implicit_fault::{Fault, arm};
use mcphost::kinds::KindRegistry;
use serde_json::json;
use std::time::Duration;

/// Every `(table, column)` that holds a tenant id: a `tenant_id` column or a
/// foreign key onto `tenants`.
fn tenant_columns(conn: &rusqlite::Connection) -> Vec<(String, String)> {
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("tables")
        .query_map([], |r| r.get(0))
        .expect("rows")
        .map(|r| r.expect("name"))
        .collect();
    let mut out = Vec::new();
    for t in tables {
        if t == "tenants" {
            continue;
        }
        let mut cols: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info(\"{t}\")"))
            .expect("info")
            .query_map([], |r| r.get::<_, String>(1))
            .expect("rows")
            .map(|r| r.expect("col"))
            .filter(|c| c == "tenant_id")
            .collect();
        let fks: Vec<String> = conn
            .prepare(&format!("PRAGMA foreign_key_list(\"{t}\")"))
            .expect("fk")
            .query_map([], |r| Ok((r.get::<_, String>(2)?, r.get::<_, String>(3)?)))
            .expect("rows")
            .map(|r| r.expect("fk"))
            .filter(|(to, _)| to == "tenants")
            .map(|(_, from)| from)
            .collect();
        cols.extend(fks);
        cols.sort();
        cols.dedup();
        out.extend(cols.into_iter().map(|c| (t.clone(), c)));
    }
    out
}

fn assert_rolled_back(server: &TestServer, expected_rollbacks: i64) {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let tenants: i64 = conn.query_row("SELECT COUNT(*) FROM tenants", [], |r| r.get(0)).expect("tenants");
    assert_eq!(tenants, 0, "the minted tenant is gone");
    let columns = tenant_columns(&conn);
    assert!(columns.len() > 5, "the schema walk finds the tenant-keyed tables: {columns:?}");
    for (table, col) in columns {
        let rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{col}\" IS NOT NULL"), [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 0, "{table}.{col} holds rows for the rolled-back tenant");
    }
    let rolled: i64 = conn
        .query_row("SELECT COUNT(*) FROM admin_events WHERE action = 'implicit_signup_rolled_back'", [], |r| r.get(0))
        .expect("rolled");
    assert_eq!(rolled, expected_rollbacks, "one implicit_signup_rolled_back row per rollback");
    let deletes: i64 = conn
        .query_row("SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_delete'", [], |r| r.get(0))
        .expect("deletes");
    assert_eq!(deletes, 0, "a rollback is not recorded as tenant_delete");
}

#[tokio::test]
async fn a_failing_timed_out_or_panicking_first_call_leaves_no_tenant_and_names_how_to_get_a_key() {
    let server = TestServer::start_full(None, KindRegistry::with_builtin(), Duration::from_millis(400), None).await;
    for (n, (name, fault, code)) in [
        ("imos-ac03-error", Fault::Error, "args_invalid"),
        ("imos-ac03-hang", Fault::Hang, "call_timeout"),
        ("imos-ac03-panic", Fault::Panic, "internal"),
    ]
    .into_iter()
    .enumerate()
    {
        arm(name, fault);
        let client = McpClient::new(&server.base_url).with_client_info(name, "1").with_user_agent(name);
        let err = client
            .tools_call("host.state.set", json!({"key": "k", "value": 1}))
            .await
            .expect_err("the injected failure reaches the caller");
        assert_eq!(err.error_code.as_deref(), Some(code), "{name}: {err:?}");
        assert_eq!(err.data["get_a_key"]["signup"], json!("signup"), "{name}: {err:?}");
        assert_eq!(err.data["get_a_key"]["quickstart"], json!("host.quickstart"), "{name}: {err:?}");
        assert_rolled_back(&server, n as i64 + 1);
    }
    assert_eq!(server.state.implicit_signup_memory.len(), 0, "no session memory survives a rollback");
}
