//! PRD-mcphost-query-diagnosis
//! AC4 — Given a successful query with rows, When its log row is read,
//! Then `diagnosis` and `hint` are null.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn successful_nonempty_query_has_null_diagnosis_and_hint() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"amount": "real"}}),
        )
        .await
        .expect("create");
    client
        .tools_call("host.table.append", json!({"table": "expenses", "rows": [{"amount": 1.0}]}))
        .await
        .expect("append");

    let result = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT amount FROM expenses"}))
            .await
            .expect("query"),
    );
    assert_eq!(result["rows"].as_array().expect("rows").len(), 1, "result: {result}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["rows"][0];
    assert!(entry["diagnosis"].is_null(), "entry: {entry}");
    assert!(entry["hint"].is_null(), "entry: {entry}");
}
