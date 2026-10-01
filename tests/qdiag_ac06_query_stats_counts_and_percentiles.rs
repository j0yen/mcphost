//! PRD-mcphost-query-diagnosis
//! AC6 — Given 40 logged queries in the last hour with 3 `row_cap`
//! refusals and 5 empty results, When `host.table.query_stats(3600)` is
//! called, Then counts match, `p95_ms` is computed from the rows, and
//! `refused.row_cap` is 3.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn query_stats_reports_matching_counts() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create");
    // mcphost_tables::ROW_CAP is 1,000 -- 1,005 rows is enough to make
    // `SELECT * FROM t` hit the row cap on every run.
    let rows: Vec<_> = (0..1_005).map(|n| json!({"x": n})).collect();
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": rows}))
        .await
        .expect("append past row cap");

    // 32 ok (non-empty) queries.
    for _ in 0..32 {
        client
            .tools_call("host.table.query", json!({"sql": "SELECT x FROM t LIMIT 5"}))
            .await
            .expect("ok query");
    }
    // 5 empty-result queries.
    for _ in 0..5 {
        client
            .tools_call("host.table.query", json!({"sql": "SELECT x FROM t WHERE x = -1"}))
            .await
            .expect("empty query");
    }
    // 3 row_cap refusals.
    for _ in 0..3 {
        client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM t"}))
            .await
            .expect_err("row cap refusal");
    }

    let stats = extract_structured(
        &client.tools_call("host.table.query_stats", json!({"window_s": 3600})).await.expect("query_stats"),
    );
    assert_eq!(stats["queries"], 40, "stats: {stats}");
    assert_eq!(stats["ok"], 32, "stats: {stats}");
    assert_eq!(stats["empty"], 5, "stats: {stats}");
    assert_eq!(stats["refused"]["table_bound_exceeded"], 3, "stats: {stats}");
    assert!(stats["p50_ms"].as_i64().is_some(), "stats: {stats}");
    assert!(stats["p95_ms"].as_i64().unwrap() >= stats["p50_ms"].as_i64().unwrap(), "stats: {stats}");
}
