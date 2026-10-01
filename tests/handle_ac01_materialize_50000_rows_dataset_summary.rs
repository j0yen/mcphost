//! PRD-mcphost-result-handles
//! AC1 -- Given a table with 50,000 rows, When `host.table.query` runs a
//! SELECT over all of it with `handle: true`, Then a `dataset-summary.v1`
//! returns with `row_count` 50000, a 20-row sample, per-column stats
//! computed over all rows (recomputed here by SQL against the source
//! table), and the call finishes under 2s on the builder.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn materialize_50000_rows_returns_dataset_summary_v1() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "ResultHandles AC1 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    // Free plan's table_rows_max (5,000) is well under this AC's 50,000
    // rows -- upgrade to pro (200,000), same convention
    // `tablemodel_ac06`'s own 50,000-row fixture already uses.
    server
        .state
        .db
        .upgrade_tenant_plan(tenant.id, "pro".to_string(), mcphost::state::rfc3339_now(), None)
        .await
        .expect("upgrade to pro");

    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create orders");

    let categories = ["a", "b", "c", "d", "e"];
    const TOTAL_ROWS: i64 = 50_000;
    const BATCH: i64 = 5_000;
    let mut start = 0;
    while start < TOTAL_ROWS {
        let rows: Vec<_> = (start..(start + BATCH).min(TOTAL_ROWS))
            .map(|i| json!({"category": categories[(i % 5) as usize], "amount": i as f64 * 0.5}))
            .collect();
        client
            .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
            .await
            .expect("append batch");
        start += BATCH;
    }

    let began = std::time::Instant::now();
    let result = client
        .tools_call(
            "host.table.query",
            json!({"sql": "SELECT * FROM orders", "handle": true}),
        )
        .await
        .expect("materialize");
    let elapsed = began.elapsed();
    assert!(elapsed.as_secs() < 2, "materialize took {elapsed:?}, must be under 2s on the builder");

    let summary = extract_structured(&result);
    assert_eq!(summary["row_count"], 50_000, "summary: {summary}");
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    assert!(handle.starts_with("hdl_"), "handle: {handle}");
    assert_eq!(summary["derived_from"], "SELECT * FROM orders", "summary: {summary}");
    assert!(summary["expires_unix"].as_i64().unwrap() > mcphost::state::now_unix(), "summary: {summary}");

    let sample = summary["sample"].as_array().expect("sample array");
    assert_eq!(sample.len(), 20, "sample_cap is 20: {summary}");
    assert_eq!(summary["sample_cap"], 20);

    // Recompute `sum` and `distinct` by SQL against the *source* table --
    // the AC's own instruction -- and compare with the handle's stats,
    // which must be computed over every row, never the 20-row sample.
    let recompute = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT SUM(amount) AS s, COUNT(DISTINCT category) AS d, COUNT(DISTINCT amount) AS da FROM orders"}),
            )
            .await
            .expect("recompute"),
    );
    let expected_sum = recompute["rows"][0]["s"].as_f64().expect("sum");
    let expected_distinct_category = recompute["rows"][0]["d"].as_i64().expect("distinct category");
    let expected_distinct_amount = recompute["rows"][0]["da"].as_i64().expect("distinct amount");

    let amount_stats = &summary["stats"]["amount"];
    assert!((amount_stats["sum"].as_f64().unwrap() - expected_sum).abs() < 0.001, "stats: {summary}");
    assert_eq!(amount_stats["distinct"], expected_distinct_amount, "stats: {summary}");
    assert!(amount_stats["mean"].is_number(), "stats: {summary}");

    let category_stats = &summary["stats"]["category"];
    assert_eq!(category_stats["distinct"], expected_distinct_category, "stats: {summary}");
    assert!(category_stats.get("sum").is_none(), "sum is only for numeric columns: {summary}");
    let top_k = category_stats["top_k"].as_array().expect("top_k array");
    assert!(!top_k.is_empty(), "stats: {summary}");

    let columns = summary["columns"].as_array().expect("columns array");
    let col_names: Vec<&str> = columns.iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(col_names.contains(&"category") && col_names.contains(&"amount"), "columns: {columns:?}");
}
