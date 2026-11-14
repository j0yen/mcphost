//! PRD-mcphost-table-sql-exec-and-grammar
//! AC5 -- Given a plan with `table_rows_max` reached, When exec runs an INSERT, Then
//! `quota_exceeded` names `table_rows_max` with the same `data` keys `table_append`
//! uses; Given two concurrent exec calls on the same row, Then both complete and
//! `changed` sums to 2.

use std::collections::BTreeSet;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("an object").keys().cloned().collect()
}

async fn count(client: &McpClient) -> i64 {
    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t"}))
        .await
        .expect("count t");
    extract_structured(&queried)["rows"][0]["n"].as_i64().expect("n")
}

#[tokio::test]
async fn an_insert_past_table_rows_max_is_quota_exceeded_with_the_keys_append_uses() {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            if plan.name == "free" {
                plan.table_rows_max = 3;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC5 Quota Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": 1}, {"id": 2, "a": 2}, {"id": 3, "a": 3}]}))
        .await
        .expect("fill the table to table_rows_max");

    let appended = client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 9, "a": 9}]}))
        .await
        .expect_err("append past the ceiling is refused");
    let exec = client
        .tools_call("host.table.exec", json!({"sql": "INSERT INTO t (id, a) VALUES (10, 10)"}))
        .await
        .expect_err("exec INSERT past the ceiling is refused");
    assert_eq!(appended.error_code.as_deref(), Some("quota_exceeded"), "{appended:?}");
    assert_eq!(exec.error_code.as_deref(), Some("quota_exceeded"), "{exec:?}");
    assert_eq!(exec.data["limit"]["name"], "table_rows_max", "{exec:?}");
    assert_eq!(exec.data["limit"]["value"], 3, "{exec:?}");
    assert_eq!(exec.data["used"], 3, "{exec:?}");
    assert_eq!(exec.data["quota"]["table_rows_max"]["remaining"], 0, "{exec:?}");
    assert_eq!(keys(&exec.data), keys(&appended.data), "exec's quota_exceeded carries the keys table_append's does");
    assert_eq!(count(&client).await, 3, "the refused INSERT wrote nothing");

    // A multi-row INSERT that would cross the ceiling is refused whole.
    let crossing = client
        .tools_call("host.table.exec", json!({"sql": "INSERT INTO t (id, a) VALUES (11, 1), (12, 2)"}))
        .await
        .expect_err("a crossing INSERT is refused");
    assert_eq!(crossing.error_code.as_deref(), Some("quota_exceeded"), "{crossing:?}");
    assert_eq!(count(&client).await, 3);

    // At the ceiling an UPDATE and a DELETE still run: they add no row.
    let updated = extract_structured(
        &client.tools_call("host.table.exec", json!({"sql": "UPDATE t SET a = 5 WHERE id = 1"})).await.expect("UPDATE at the ceiling"),
    );
    assert_eq!(updated["changed"], 1, "{updated}");
    let deleted = extract_structured(
        &client.tools_call("host.table.exec", json!({"sql": "DELETE FROM t WHERE id = 3"})).await.expect("DELETE at the ceiling"),
    );
    assert_eq!(deleted["changed"], 1, "{deleted}");
    // ... and the freed row lets an INSERT in again.
    client.tools_call("host.table.exec", json!({"sql": "INSERT INTO t (id, a) VALUES (13, 13)"})).await.expect("INSERT below the ceiling");
    assert_eq!(count(&client).await, 3);
}

#[tokio::test]
async fn two_concurrent_execs_on_one_row_both_complete_and_changed_sums_to_two() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC5 Race Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client.tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": 0}]})).await.expect("seed");

    let other = McpClient::with_bearer(&server.base_url, &key);
    for round in 1..=3 {
        let sql = json!({"sql": "UPDATE t SET a = a + 1 WHERE id = 1"});
        let (first, second) =
            tokio::join!(client.tools_call("host.table.exec", sql.clone()), other.tools_call("host.table.exec", sql));
        let changed = extract_structured(&first.expect("first exec completes"))["changed"].as_i64().expect("changed")
            + extract_structured(&second.expect("second exec completes"))["changed"].as_i64().expect("changed");
        assert_eq!(changed, 2, "round {round}: both calls changed the row");
        let a = extract_structured(&client.tools_call("host.table.query", json!({"sql": "SELECT a FROM t WHERE id = 1"})).await.expect("read"));
        assert_eq!(a["rows"][0]["a"], 2 * round, "round {round}: neither update was lost");
    }
}
