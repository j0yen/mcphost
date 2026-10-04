//! PRD-mcphost-result-handles
//! AC12 -- Given an empty result, When materialised, Then the handle
//! exists with `row_count` 0, an empty sample, and stats with `distinct`
//! 0 per column.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn empty_result_materializes_with_zero_row_count_and_distinct() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC12 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "readings", "columns": {"sensor": "text", "value": "real"}}),
        )
        .await
        .expect("create readings");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "readings", "rows": [{"sensor": "a", "value": 1.0}]}),
        )
        .await
        .expect("append one row");

    let summary = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT * FROM readings WHERE value > 100", "handle": true}),
            )
            .await
            .expect("materialize an empty result"),
    );

    assert_eq!(summary["row_count"], 0, "summary: {summary}");
    assert_eq!(summary["sample"].as_array().expect("sample array").len(), 0, "summary: {summary}");
    assert!(summary["bytes"].as_i64().expect("bytes") >= 0, "summary: {summary}");
    assert!(summary["expires_unix"].as_i64().unwrap() > 0, "summary: {summary}");

    let sensor_stats = &summary["stats"]["sensor"];
    assert_eq!(sensor_stats["distinct"], 0, "sensor stats: {sensor_stats}");
    assert!(sensor_stats["min"].is_null(), "sensor stats: {sensor_stats}");
    assert!(sensor_stats["max"].is_null(), "sensor stats: {sensor_stats}");
    assert_eq!(sensor_stats["top_k"].as_array().unwrap().len(), 0, "sensor stats: {sensor_stats}");

    let value_stats = &summary["stats"]["value"];
    assert_eq!(value_stats["distinct"], 0, "value stats: {value_stats}");
    assert!(value_stats["sum"].is_null(), "value stats: {value_stats}");
    assert!(value_stats["mean"].is_null(), "value stats: {value_stats}");

    // The handle really exists (as an empty table), queryable like any
    // other.
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    let count = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": format!("SELECT COUNT(*) AS n FROM {handle}")}))
            .await
            .expect("query the empty handle"),
    );
    assert_eq!(count["rows"][0]["n"], 0, "count: {count}");
}
