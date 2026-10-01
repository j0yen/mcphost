//! PRD-mcphost-table-context-and-sql-passthrough
//! AC7 — Given a tenant with 1,000 log rows, When one more query runs,
//! Then the log holds 1,000 rows, the oldest is gone, and
//! `host.table.schema`'s `bytes_used` includes the log.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn log_evicts_oldest_past_1000_rows_and_counts_toward_bytes_used() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create");

    let bytes_before = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "t"})).await.expect("schema before"),
    )["bytes_used"]
        .as_i64()
        .expect("bytes_used before");

    // 1,000 distinct queries, then one more (1,001st) -- the eviction this
    // AC names.
    for n in 1..=1_001 {
        client
            .tools_call("host.table.query", json!({"sql": format!("SELECT {n} AS n FROM t")}))
            .await
            .unwrap_or_else(|e| panic!("query {n} failed: {e:?}"));
    }

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 200})).await.expect("query_log"),
    );
    // query_log itself is capped at 200 per page, not 1,000 -- walk every
    // page to count the whole log.
    let mut total = log["entries"].as_array().expect("entries").len();
    let mut oldest_id = log["entries"].as_array().unwrap().last().unwrap()["id"].as_i64().unwrap();
    loop {
        let page = extract_structured(
            &client
                .tools_call("host.table.query_log", json!({"limit": 200, "before_id": oldest_id}))
                .await
                .expect("query_log page"),
        );
        let entries = page["entries"].as_array().expect("entries");
        if entries.is_empty() {
            break;
        }
        total += entries.len();
        oldest_id = entries.last().unwrap()["id"].as_i64().unwrap();
    }
    assert_eq!(total, 1_000, "query log must hold exactly 1,000 rows, got {total}");

    // The very first query ("SELECT 1 AS n FROM t") must be gone; the
    // last one ("SELECT 1001 AS n FROM t") must be present.
    let newest = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("newest"),
    );
    assert_eq!(newest["entries"][0]["sql"], "SELECT 1001 AS n FROM t", "newest: {newest}");

    let bytes_after = extract_structured(
        &client.tools_call("host.table.schema", json!({"table": "t"})).await.expect("schema after"),
    )["bytes_used"]
        .as_i64()
        .expect("bytes_used after");
    assert!(
        bytes_after > bytes_before,
        "bytes_used must grow as the query log fills (before={bytes_before}, after={bytes_after})"
    );
}
