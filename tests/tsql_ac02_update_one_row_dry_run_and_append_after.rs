//! PRD-mcphost-table-sql-exec-and-grammar
//! AC2 -- Given a table with 3 rows, When
//! `host.table.exec(sql="UPDATE t SET a=1 WHERE id=2")` runs, Then the response is
//! `{changed: 1, table: "t"}`, a following `host.table.query` shows the change,
//! `bump_change_counter` ran with `rewrite=true`, a `dry_run` call changes nothing,
//! and a `host.table.append` right after succeeds (`query_only` not left on).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// `(change_counter, rewrite_counter)` of `table` in the main database.
fn counters(server: &TestServer, table: &str) -> (i64, i64) {
    let conn = rusqlite::Connection::open(server.state.db.path()).expect("open the main db");
    conn.query_row(
        "SELECT change_counter, rewrite_counter FROM table_change_counters WHERE table_name = ?1",
        [table],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((0, 0))
}

async fn rows_of(client: &McpClient) -> Vec<Value> {
    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT id, a FROM t ORDER BY id"}))
        .await
        .expect("query t");
    extract_structured(&queried)["rows"].as_array().expect("rows").clone()
}

#[tokio::test]
async fn update_changes_one_row_bumps_with_rewrite_dry_run_writes_nothing_and_append_follows() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": 10}, {"id": 2, "a": 20}, {"id": 3, "a": 30}]}))
        .await
        .expect("append 3 rows");
    let before = counters(&server, "t");

    let result = extract_structured(
        &client
            .tools_call("host.table.exec", json!({"sql": "UPDATE t SET a=1 WHERE id=2"}))
            .await
            .expect("exec UPDATE"),
    );
    assert_eq!(result["changed"], 1, "{result}");
    assert_eq!(result["table"], "t", "{result}");

    let rows = rows_of(&client).await;
    assert_eq!(rows, vec![json!({"id": 1, "a": 10}), json!({"id": 2, "a": 1}), json!({"id": 3, "a": 30})], "the UPDATE shows");

    let after = counters(&server, "t");
    assert_eq!(after.0, before.0 + 1, "one exec bumps change_counter once: {before:?} -> {after:?}");
    assert_eq!(after.1, before.1 + 1, "an UPDATE bumps with rewrite=true: {before:?} -> {after:?}");

    // An INSERT is an append-shaped write: change_counter moves, rewrite_counter does not.
    client.tools_call("host.table.exec", json!({"sql": "INSERT INTO t (id, a) VALUES (4, 40)"})).await.expect("exec INSERT");
    let inserted = counters(&server, "t");
    assert_eq!(inserted.0, after.0 + 1, "{after:?} -> {inserted:?}");
    assert_eq!(inserted.1, after.1, "an INSERT bumps with rewrite=false: {after:?} -> {inserted:?}");

    // dry_run: answers the would-be changed count and writes nothing.
    let dry = extract_structured(
        &client
            .tools_call("host.table.exec", json!({"sql": "UPDATE t SET a=99 WHERE id=3", "dry_run": true}))
            .await
            .expect("dry_run exec"),
    );
    assert_eq!(dry["_meta"]["dry_run"], true, "{dry}");
    assert_eq!(dry["would"]["rows"][0]["changed"], 1, "the would-be changed count: {dry}");
    assert_eq!(dry["would"]["rows"][0]["table"], "t", "{dry}");
    assert_eq!(rows_of(&client).await[2], json!({"id": 3, "a": 30}), "a dry run changes nothing");
    assert_eq!(counters(&server, "t"), inserted, "a dry run does not bump the counters");

    // query_only is not left on: a write right after a read and an exec succeeds.
    let appended = extract_structured(
        &client
            .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 5, "a": 50}]}))
            .await
            .expect("append right after exec"),
    );
    assert_eq!(appended["appended"], 1, "{appended}");
    let again = extract_structured(
        &client.tools_call("host.table.exec", json!({"sql": "DELETE FROM t WHERE id = 5"})).await.expect("exec right after append"),
    );
    assert_eq!(again["changed"], 1, "{again}");
}
