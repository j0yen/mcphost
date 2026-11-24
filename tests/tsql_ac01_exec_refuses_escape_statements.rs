//! PRD-mcphost-table-sql-exec-and-grammar
//! AC1 -- Given `host.table.exec` statements containing `ATTACH`, `PRAGMA`,
//! `load_extension(...)`, `readfile(...)`, a `CREATE TRIGGER`, or an INSERT whose
//! subquery is not a plain SELECT, When each runs, Then each is refused
//! `table_exec_rejected` and the tenant's file, schema and pragmas are unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

/// Everything about the tenant's table file a refused statement could have
/// changed: its schema text, its rows, its pragmas, and the files beside it.
fn fingerprint(data_dir: &std::path::Path, tenant_id: i64) -> serde_json::Value {
    let tables_dir = data_dir.join("tables");
    let path = tables_dir.join(format!("{tenant_id}.db"));
    let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open the tenant file read-only");
    let schema: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let rows: Vec<(i64, i64)> = conn
        .prepare("SELECT id, a FROM t ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let pragma = |name: &str| -> String {
        conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| format!("{v:?}"))
            .unwrap_or_default()
    };
    let mut files: Vec<String> = std::fs::read_dir(&tables_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
        .collect();
    files.sort();
    json!({
        "schema": schema,
        "rows": rows,
        "user_version": pragma("user_version"),
        "journal_mode": pragma("journal_mode"),
        "foreign_keys": pragma("foreign_keys"),
        "auto_vacuum": pragma("auto_vacuum"),
        "files": files,
    })
}

#[tokio::test]
async fn exec_refuses_every_statement_that_reaches_past_the_tenants_tables() {
    let server = TestServer::start().await;
    let (namespace, key) = signup(&server.base_url, "Tsql AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant_id = server.state.db.find_tenant_by_namespace(namespace).await.unwrap().expect("tenant row").id;

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": 1}, {"id": 2, "a": 2}, {"id": 3, "a": 3}]}))
        .await
        .expect("append rows");

    let escape = server.data_dir.0.join("tsql-ac1-escape.db");
    let attach = format!("ATTACH DATABASE '{}' AS evil", escape.display());
    let statements: Vec<(&str, String, &str)> = vec![
        ("ATTACH", attach, "ATTACH is not allowed"),
        ("PRAGMA", "PRAGMA query_only = OFF".to_string(), "PRAGMA is not allowed"),
        ("PRAGMA journal_mode", "PRAGMA journal_mode = DELETE".to_string(), "PRAGMA is not allowed"),
        (
            "load_extension",
            "INSERT INTO t (id, a) VALUES (10, load_extension('/lib/evil.so'))".to_string(),
            "load_extension",
        ),
        ("readfile", "INSERT INTO t (id, a) SELECT 11, length(readfile('/etc/passwd'))".to_string(), "readfile"),
        (
            "readfile uppercase",
            "UPDATE t SET a = length(READFILE('/etc/hostname')) WHERE id = 1".to_string(),
            "readfile",
        ),
        (
            "CREATE TRIGGER",
            "CREATE TRIGGER wipe AFTER INSERT ON t BEGIN DELETE FROM t; END".to_string(),
            "CREATE TRIGGER is not allowed",
        ),
        ("CREATE TABLE", "CREATE TABLE sneaky (x integer)".to_string(), "CREATE TABLE is not allowed"),
        ("DROP TABLE", "DROP TABLE t".to_string(), "DROP TABLE is not allowed"),
        (
            "INSERT subquery that deletes",
            "INSERT INTO t (id, a) WITH gone AS (DELETE FROM t RETURNING id) SELECT id + 100, 1 FROM gone".to_string(),
            "plain SELECT",
        ),
        ("INSERT TABLE subquery", "INSERT INTO t (id, a) TABLE t".to_string(), "plain SELECT"),
        (
            "table function pragma_table_info",
            "INSERT INTO t (id, a) SELECT cid + 50, 1 FROM pragma_table_info('t')".to_string(),
            "pragma_table_info",
        ),
        ("reserved table", "DELETE FROM _mcphost_meta WHERE name = 't'".to_string(), "reserved table"),
        (
            "reserved table read",
            "INSERT INTO t (id, a) SELECT 12, count(*) FROM _mcphost_query_log".to_string(),
            "reserved table",
        ),
        (
            "sqlite_master read",
            "INSERT INTO t (id, a) SELECT 13, count(*) FROM sqlite_master".to_string(),
            "reserved table",
        ),
        ("schema prefix", "DELETE FROM main.t WHERE id = 1".to_string(), "no schema or database prefix"),
        (
            "two statements",
            "DELETE FROM t WHERE id = 1; DELETE FROM t WHERE id = 2".to_string(),
            "multiple statements",
        ),
        ("SELECT sent to exec", "SELECT * FROM t".to_string(), "host.table.query"),
    ];

    let before = fingerprint(&server.data_dir.0, tenant_id);
    for (label, sql, expected) in &statements {
        let err = client
            .tools_call("host.table.exec", json!({"sql": sql}))
            .await
            .expect_err(&format!("{label}: must be refused: {sql}"));
        assert_eq!(err.error_code.as_deref(), Some("table_exec_rejected"), "{label}: error: {err:?}");
        let reason = err.data["reason"].as_str().unwrap_or_default();
        assert!(reason.contains(expected), "{label}: data.reason should mention {expected:?}: {err:?}");
        assert!(
            err.data["allowed"].as_array().is_some_and(|a| !a.is_empty()),
            "{label}: data.allowed lists the statement forms: {err:?}"
        );
    }
    assert!(!escape.exists(), "ATTACH must not have created {escape:?}");
    assert_eq!(fingerprint(&server.data_dir.0, tenant_id), before, "a refused statement changed the tenant's file");

    // The verb itself works: a plain INSERT is not caught by the gate.
    let ok = extract_structured(
        &client
            .tools_call("host.table.exec", json!({"sql": "INSERT INTO t (id, a) VALUES (7, 7)"}))
            .await
            .expect("a plain INSERT runs"),
    );
    assert_eq!(ok["changed"], 1, "control: {ok}");
    assert_eq!(ok["table"], "t", "control: {ok}");
}
