//! PRD-mcphost-table-context-and-sql-passthrough
//! AC11 — Given ten `host.table.query` calls issued concurrently for one
//! tenant, When they finish, Then the log holds exactly ten new rows and
//! every one has a `duration_ms`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ten_concurrent_queries_produce_ten_log_rows_each_with_duration() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC11 Tenant").await;
    let setup_client = McpClient::with_bearer(&server.base_url, &key);
    setup_client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"n": "integer"}}))
        .await
        .expect("create t");
    setup_client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"n": 1}]}))
        .await
        .expect("append one row");

    const CONCURRENCY: usize = 10;
    let mut handles = Vec::with_capacity(CONCURRENCY);
    for i in 0..CONCURRENCY {
        let base_url = server.base_url.clone();
        let key = key.clone();
        handles.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            client
                .tools_call("host.table.query", json!({"sql": format!("SELECT {i} AS marker FROM t")}))
                .await
                .unwrap_or_else(|e| panic!("concurrent query {i}: {} {}", e.code, e.message));
        }));
    }
    for h in handles {
        h.await.expect("concurrent query task panicked");
    }

    let log = extract_structured(
        &setup_client.tools_call("host.table.query_log", json!({"limit": 50})).await.expect("query_log"),
    );
    let rows = log["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), CONCURRENCY, "expected exactly {CONCURRENCY} new log rows: {log}");
    for r in rows {
        assert!(r["duration_ms"].is_number(), "every log row must carry duration_ms: {r}");
        assert!(r["error_code"].is_null(), "every concurrent query must have succeeded: {r}");
    }
}
