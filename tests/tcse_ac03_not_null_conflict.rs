//! PRD-mcphost-table-conflict-structured-error
//! AC3 -- Given a table with a `NOT NULL` column that schema validation does
//! not reject, When a row without it is appended, Then the code is
//! `table_row_conflict` with `constraint: "not_null"` and `column` named.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tcse_ac03() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tcse AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "notes", "columns": {"id": "integer", "body": "text"}}))
        .await
        .expect("table create ok");

    // The tool API declares no NOT NULL columns, so harden the tenant's own
    // table file directly: schema validation still accepts a row without
    // `body`, and SQLite is the one that refuses it.
    let db_file = std::fs::read_dir(server.data_dir.0.join("tables"))
        .expect("tables dir")
        .map(|e| e.expect("dir entry").path())
        .find(|p| p.extension().is_some_and(|x| x == "db"))
        .expect("tenant table db");
    let conn = rusqlite::Connection::open(&db_file).expect("open tenant db");
    conn.execute_batch("DROP TABLE notes; CREATE TABLE notes (id INTEGER, body TEXT NOT NULL);")
        .expect("recreate with NOT NULL");
    drop(conn);

    let err = client
        .tools_call("host.table.append", json!({"table": "notes", "rows": [{"id": 1}]}))
        .await
        .expect_err("missing NOT NULL value must fail");
    assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{err:?}");
    assert_eq!(err.data["constraint"], json!("not_null"), "{err:?}");
    assert_eq!(err.data["column"], json!("body"), "{err:?}");
    assert_eq!(err.data["table"], json!("notes"), "{err:?}");
}
