//! PRD-mcphost-result-handles
//! AC12 -- Given an empty result, When materialized, Then the handle
//! exists with `row_count` 0, an empty sample, and stats with `distinct`
//! 0 per column.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn empty_result_materializes_a_zero_row_handle() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC12 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": [{"category": "a", "amount": 1.0}]}))
        .await
        .expect("append one row (so the table isn't itself empty)");

    let materialize = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT * FROM orders WHERE category = 'does-not-exist'", "handle": true}),
            )
            .await
            .expect("materialize empty result"),
    );

    assert_eq!(materialize["row_count"], 0, "summary: {materialize}");
    let sample = materialize["sample"].as_array().expect("sample array");
    assert!(sample.is_empty(), "summary: {materialize}");

    let category_stats = &materialize["stats"]["category"];
    assert_eq!(category_stats["distinct"], 0, "summary: {materialize}");
    assert_eq!(category_stats["min"], serde_json::Value::Null, "summary: {materialize}");

    let amount_stats = &materialize["stats"]["amount"];
    assert_eq!(amount_stats["distinct"], 0, "summary: {materialize}");
    assert_eq!(amount_stats["sum"], serde_json::Value::Null, "summary: {materialize}");

    // The handle itself still exists and is queryable -- an empty result
    // is a real (zero-row) handle, not a refusal.
    let handle = materialize["handle"].as_str().expect("handle").to_string();
    let requery = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": format!("SELECT COUNT(*) AS n FROM {handle}")}))
            .await
            .expect("requery empty handle"),
    );
    assert_eq!(requery["rows"][0]["n"], 0, "requery: {requery}");
}
