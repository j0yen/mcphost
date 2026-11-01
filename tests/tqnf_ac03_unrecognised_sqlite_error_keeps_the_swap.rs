//! PRD-mcphost-table-query-names-missing-table
//! AC3 -- Given a rusqlite error `parse_bind_error` does not recognise (a
//! tenant database file overwritten with non-SQLite bytes, so `prepare`
//! returns `file is not a database`; works as root too), When
//! `host.table.query` runs a valid `SELECT` on a declared table, Then the
//! reply is the internal-error swap with a `request_id` and no table-family
//! `data.error_code` (R3 guardrail).

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tqnf_ac03() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Tqnf AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer", "total": "real"}}))
        .await
        .expect("create");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant row");
    let db_file = server.state.db.data_dir().join("tables").join(format!("{}.db", tenant.id));
    assert!(db_file.is_file(), "tenant table file must exist: {db_file:?}");
    for sidecar in ["-wal", "-shm"] {
        let mut p = db_file.clone().into_os_string();
        p.push(sidecar);
        let _ = std::fs::remove_file(p);
    }
    std::fs::write(&db_file, vec![b'x'; 8192]).expect("overwrite with non-SQLite bytes");

    let err = client
        .tools_call("host.table.query", json!({"sql": "SELECT id FROM orders"}))
        .await
        .expect_err("a corrupt database file is a host fault");
    assert_eq!(err.code, -32603, "{err:?}");
    assert!(err.message.starts_with("internal error; request_id="), "{}", err.message);
    assert!(err.data["request_id"].as_str().is_some_and(|id| !id.is_empty()), "{err:?}");
    let code = err.error_code.as_deref().unwrap_or_default();
    assert!(!code.starts_with("table_"), "table-family code on an unrecognised error: {err:?}");
    assert!(err.data.get("candidates").is_none() && err.data.get("table").is_none(), "{err:?}");
}
