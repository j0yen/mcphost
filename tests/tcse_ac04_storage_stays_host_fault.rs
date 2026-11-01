//! PRD-mcphost-table-conflict-structured-error
//! AC4 -- Given `AppError::Storage("db lock poisoned")`, When mapped, Then the
//! code is `storage`, the JSON-RPC code `-32603`, and `HOST_FAULT_CODES`
//! contains `storage` and not `table_row_conflict`.

use mcphost::errors::{AppError, HOST_FAULT_CODES};

#[test]
fn tcse_ac04() {
    let err = AppError::Storage("db lock poisoned".into());
    assert_eq!(err.code(), "storage");
    let data = err.into_error_data();
    assert_eq!(data.code.0, -32603, "{data:?}");
    assert_eq!(data.data.as_ref().and_then(|d| d["error_code"].as_str()), Some("storage"));

    assert!(HOST_FAULT_CODES.contains(&"storage"));
    assert!(!HOST_FAULT_CODES.contains(&mcphost::tables::TABLE_ROW_CONFLICT));

    // A real constraint failure through the one `From<rusqlite::Error>` site
    // is the caller's fault: `-32602`, never a member of the host-fault set.
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
    conn.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY); INSERT INTO t VALUES (1);").expect("seed");
    let sqlite_err = conn.execute("INSERT INTO t VALUES (1)", []).expect_err("duplicate key");
    let conflict = AppError::from(sqlite_err);
    assert_eq!(conflict.code(), mcphost::tables::TABLE_ROW_CONFLICT);
    assert_eq!(conflict.into_error_data().code.0, -32602);
}
