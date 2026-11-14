//! PRD-mcphost-table-sql-exec-and-grammar
//! AC3 -- Given `DELETE FROM t` without WHERE, When exec runs, Then it is refused
//! `table_exec_rejected` with `data.allowed` containing `DELETE FROM <table> WHERE …`
//! and no row is deleted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn count(client: &McpClient) -> i64 {
    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t"}))
        .await
        .expect("count t");
    extract_structured(&queried)["rows"][0]["n"].as_i64().expect("n")
}

#[tokio::test]
async fn delete_and_update_without_where_are_refused_from_the_ast_and_delete_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "text"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": "x"}, {"id": 2, "a": "y"}, {"id": 3, "a": "z"}]}))
        .await
        .expect("append 3 rows");

    let err = client
        .tools_call("host.table.exec", json!({"sql": "DELETE FROM t"}))
        .await
        .expect_err("a WHERE-less DELETE must be refused");
    assert_eq!(err.error_code.as_deref(), Some("table_exec_rejected"), "{err:?}");
    let allowed: Vec<&str> = err.data["allowed"].as_array().expect("data.allowed").iter().filter_map(Value::as_str).collect();
    assert!(allowed.contains(&"DELETE FROM <table> WHERE …"), "data.allowed names the DELETE form: {allowed:?}");
    assert!(
        allowed.iter().any(|form| form.starts_with("UPDATE ") && form.contains("WHERE")),
        "data.allowed names the UPDATE form: {allowed:?}"
    );
    assert!(err.data["reason"].as_str().is_some_and(|r| r.contains("WHERE")), "the reason names the missing clause: {err:?}");
    assert_eq!(count(&client).await, 3, "no row is deleted");

    // The gate reads the AST: the word WHERE in a comment or a string literal is not a WHERE clause.
    for sql in [
        "DELETE FROM t -- WHERE id = 1",
        "DELETE FROM t /* WHERE id = 1 */",
        "UPDATE t SET a = 'WHERE id = 1'",
        "UPDATE t SET a = 'q'",
    ] {
        let err = client.tools_call("host.table.exec", json!({"sql": sql})).await.expect_err(sql);
        assert_eq!(err.error_code.as_deref(), Some("table_exec_rejected"), "{sql}: {err:?}");
    }
    assert_eq!(count(&client).await, 3, "no refused statement changed a row");
    let untouched = extract_structured(
        &client.tools_call("host.table.query", json!({"sql": "SELECT a FROM t ORDER BY id"})).await.expect("read a"),
    );
    assert_eq!(untouched["rows"], json!([{"a": "x"}, {"a": "y"}, {"a": "z"}]));

    // Control: the same DELETE with a WHERE runs.
    let ok = extract_structured(
        &client.tools_call("host.table.exec", json!({"sql": "DELETE FROM t WHERE id = 2"})).await.expect("DELETE ... WHERE"),
    );
    assert_eq!(ok["changed"], 1, "{ok}");
    assert_eq!(count(&client).await, 2);
}
