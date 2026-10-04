//! PRD-mcphost-query-diagnosis
//! AC8 (P1) — Given a 200 KB result, When logged, Then `result_bytes` is
//! within 1% of the serialised size and `est_tokens` equals
//! `ceil(result_bytes / 4)`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn large_result_logs_result_bytes_and_est_tokens() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "text"}}))
        .await
        .expect("create");
    // 1,000-char string per row; ~300 rows clears 200KB of serialized JSON
    // once the row/column wrapping overhead is counted, and stays well
    // under host.table.query's 1,000-row cap.
    let padding = "a".repeat(1_000);
    let rows: Vec<_> = (0..300).map(|_| json!({"x": padding})).collect();
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": rows}))
        .await
        .expect("append");

    let result = extract_structured(
        &client.tools_call("host.table.query", json!({"sql": "SELECT x FROM t"})).await.expect("query"),
    );
    let actual_serialized_bytes = serde_json::to_string(&result["rows"]).unwrap().len() as i64;
    assert!(actual_serialized_bytes > 200_000, "fixture must exceed 200KB: {actual_serialized_bytes}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["rows"][0];
    let result_bytes = entry["result_bytes"].as_i64().expect("result_bytes");
    let est_tokens = entry["est_tokens"].as_i64().expect("est_tokens");

    let diff_pct = ((result_bytes - actual_serialized_bytes).abs() as f64) / (actual_serialized_bytes as f64) * 100.0;
    assert!(
        diff_pct <= 1.0,
        "result_bytes {result_bytes} must be within 1% of the actual serialized size {actual_serialized_bytes} (diff {diff_pct}%)"
    );
    assert_eq!(est_tokens, (result_bytes + 3) / 4, "est_tokens must be ceil(result_bytes / 4)");
}
