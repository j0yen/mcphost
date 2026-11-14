//! PRD-mcphost-table-sql-exec-and-grammar
//! AC6 -- Given `host.table.find(table="t", where={"amount": {">": 500}, "status": "open"})`,
//! When it runs, Then rows match the parameterised SELECT and `data.sql` shows it; Given
//! an unknown operator `"~"`, Then the refusal lists `allowed_operators` and the table's
//! columns.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::table::find::ALLOWED_OPERATORS;
use serde_json::{Value, json};

async fn seeded() -> (TestServer, McpClient) {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "t", "columns": {"id": "integer", "amount": "real", "status": "text"}, "primary_key": "id"}),
        )
        .await
        .expect("create t");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "t", "rows": [
                {"id": 1, "amount": 100.0, "status": "open"},
                {"id": 2, "amount": 600.0, "status": "open"},
                {"id": 3, "amount": 700.0, "status": "closed"},
                {"id": 4, "amount": 800.0, "status": "open"},
                {"id": 5, "amount": 400.0, "status": "open"},
            ]}),
        )
        .await
        .expect("append rows");
    (server, client)
}

fn ids(result: &Value) -> Vec<i64> {
    result["rows"].as_array().expect("rows").iter().map(|r| r["id"].as_i64().expect("id")).collect()
}

#[tokio::test]
async fn find_compiles_the_example_to_a_parameterised_select_and_shows_it() {
    let (_server, client) = seeded().await;
    let found = extract_structured(
        &client
            .tools_call(
                "host.table.find",
                json!({"table": "t", "where": {"amount": {">": 500}, "status": "open"}, "order": "id"}),
            )
            .await
            .expect("find"),
    );
    assert_eq!(ids(&found), vec![2, 4], "{found}");

    let sql = found["data"]["sql"].as_str().expect("data.sql");
    assert!(sql.starts_with("SELECT * FROM \"t\" WHERE \"amount\" > :ex0 AND \"status\" = :ex1"), "{sql}");
    assert!(!sql.contains("500") && !sql.contains("'open'"), "values are bound, never spliced into the text: {sql}");
    assert_eq!(found["data"]["params"], json!({":ex0": 500, ":ex1": "open"}), "{found}");
    assert_eq!(found["total"], 2, "the page total counts the matches: {found}");

    // The same rows the equivalent hand-written SELECT returns.
    let by_sql = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT * FROM t WHERE amount > 500 AND status = 'open' ORDER BY id"}),
            )
            .await
            .expect("query"),
    );
    assert_eq!(found["rows"], by_sql["rows"], "find matches the SELECT it compiled");

    // find rides the query log like any other SELECT.
    let log = extract_structured(&client.tools_call("host.table.query_log", json!({"limit": 3})).await.expect("log"));
    assert!(
        log["rows"].as_array().expect("rows").iter().any(|r| r["sql"].as_str().is_some_and(|s| s.contains(":ex0"))),
        "the compiled statement is in the query log: {log}"
    );
}

#[tokio::test]
async fn find_takes_in_like_null_order_and_limit_and_binds_hostile_values() {
    let (_server, client) = seeded().await;
    let call = |args: Value| {
        let client = &client;
        async move { extract_structured(&client.tools_call("host.table.find", args).await.expect("find")) }
    };
    assert_eq!(ids(&call(json!({"table": "t", "where": {"id": [1, 3, 5]}, "order": "id"})).await), vec![1, 3, 5]);
    assert_eq!(ids(&call(json!({"table": "t", "where": {"status": {"like": "clo%"}}})).await), vec![3]);
    assert_eq!(ids(&call(json!({"table": "t", "where": {"status": {"!=": null}}, "order": "id desc", "limit": 2})).await), vec![5, 4]);
    assert_eq!(ids(&call(json!({"table": "t", "order": ["status", "id desc"], "limit": 2})).await), vec![3, 5]);
    assert_eq!(ids(&call(json!({"table": "t"})).await).len(), 5, "no where matches every row");
    assert_eq!(ids(&call(json!({"table": "t", "where": {"status": "open' OR '1'='1"}})).await), Vec::<i64>::new());
    assert_eq!(ids(&call(json!({"table": "t", "where": {"amount": {">=": 400, "<=": 700}}, "order": "id"})).await), vec![2, 3, 5]);
    assert_eq!(ids(&call(json!({"table": "t", "where": {"id": {"!=": 1}, "status": {"=": "open"}}, "order": "id"})).await), vec![2, 4, 5]);
}

#[tokio::test]
async fn an_unknown_operator_column_or_order_is_refused_with_the_operators_and_the_columns() {
    let (_server, client) = seeded().await;
    let columns = json!(["amount", "id", "status"]);

    let err = client
        .tools_call("host.table.find", json!({"table": "t", "where": {"amount": {"~": 500}}}))
        .await
        .expect_err("an unknown operator is refused");
    assert_eq!(err.error_code.as_deref(), Some("table_query_rejected"), "{err:?}");
    assert_eq!(err.data["allowed_operators"], json!(ALLOWED_OPERATORS), "{err:?}");
    assert_eq!(err.data["columns"], columns, "{err:?}");
    assert_eq!(err.data["operator"], "~", "{err:?}");
    assert_eq!(err.data["column"], "amount", "{err:?}");

    let unknown_column = client
        .tools_call("host.table.find", json!({"table": "t", "where": {"price": 5}}))
        .await
        .expect_err("an unknown column is refused");
    assert_eq!(unknown_column.error_code.as_deref(), Some("table_query_rejected"), "{unknown_column:?}");
    assert_eq!(unknown_column.data["columns"], columns, "{unknown_column:?}");
    assert_eq!(unknown_column.data["allowed_operators"], json!(ALLOWED_OPERATORS));

    let bad_order = client
        .tools_call("host.table.find", json!({"table": "t", "order": "nope desc"}))
        .await
        .expect_err("an unknown order column is refused");
    assert_eq!(bad_order.data["columns"], columns, "{bad_order:?}");

    let no_table = client
        .tools_call("host.table.find", json!({"table": "missing", "where": {"id": 1}}))
        .await
        .expect_err("an undeclared table is table_not_found");
    assert_eq!(no_table.error_code.as_deref(), Some("table_not_found"), "{no_table:?}");
}
