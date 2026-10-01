//! PRD-mcphost-table-context-and-sql-passthrough
//! AC11 (P1) — Given ten `host.table.query` calls issued concurrently for
//! one tenant, When they finish, Then the log holds exactly ten new rows
//! and every one has a `duration_ms`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ten_concurrent_queries_all_produce_a_logged_row() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SqlPass AC11 Tenant").await;
    let seed_client = McpClient::with_bearer(&server.base_url, &key);
    seed_client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create");
    seed_client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"x": 1}]}))
        .await
        .expect("append");

    const N: usize = 10;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        let base_url = server.base_url.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            client
                .tools_call("host.table.query", json!({"sql": "SELECT * FROM t"}))
                .await
                .expect("concurrent query")
        }));
    }
    for task in tasks {
        task.await.expect("task must not panic");
    }

    let log = extract_structured(
        &seed_client.tools_call("host.table.query_log", json!({"limit": 50})).await.expect("query_log"),
    );
    let entries = log["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), N, "expected exactly {N} logged rows, got {}: {log}", entries.len());
    for entry in entries {
        assert!(entry["duration_ms"].as_i64().is_some(), "every row needs a duration_ms: {entry}");
        assert_eq!(entry["row_count"], 1, "entry: {entry}");
    }
}
