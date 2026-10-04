//! PRD-mcphost-query-diagnosis
//! AC11 (P1) — Given ten concurrent refused queries, When they finish,
//! Then all ten log rows carry a hint and none is missing a diagnosis.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ten_concurrent_refused_queries_all_carry_a_hint() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC11 Tenant").await;
    let seed_client = McpClient::with_bearer(&server.base_url, &key);
    seed_client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");

    const N: usize = 10;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        let base_url = server.base_url.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base_url, &key);
            client
                .tools_call("host.table.query", json!({"sql": "SELECT amout FROM expenses"}))
                .await
                .expect_err("amout is a typo")
        }));
    }
    for task in tasks {
        task.await.expect("task must not panic");
    }

    let log = extract_structured(
        &seed_client.tools_call("host.table.query_log", json!({"limit": 50})).await.expect("query_log"),
    );
    let entries = log["rows"].as_array().expect("entries");
    assert_eq!(entries.len(), N, "expected exactly {N} logged rows, got {}: {log}", entries.len());
    for entry in entries {
        assert!(!entry["diagnosis"].is_null(), "every row needs a diagnosis: {entry}");
        assert!(entry["hint"].as_str().is_some(), "every row needs a hint: {entry}");
    }
}
