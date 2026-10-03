//! PRD-mcphost-result-handles
//! AC1 -- Given a table with 50,000 rows, When `host.table.query` runs a
//! SELECT over all of it with `handle: true`, Then a `dataset-summary.v1`
//! returns with `row_count` 50000, a 20-row sample, per-column stats
//! computed over all rows (this test recomputes `sum` and `distinct` by
//! SQL), and the call finishes under 2 s on the builder.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn materialize_50000_rows_returns_honest_summary_under_2s() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Handle AC1 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    // Free plan's table_rows_max (5,000) is well under this AC's 50,000
    // rows -- upgrade to pro (200,000) so the append itself isn't what
    // refuses the fixture.
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
            json!({"name": "big", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create big");

    const TOTAL_ROWS: i64 = 50_000;
    const BATCH: i64 = 5_000;
    let categories = ["a", "b", "c", "d", "e"];
    let mut start = 0;
    while start < TOTAL_ROWS {
        let rows: Vec<_> = (start..(start + BATCH).min(TOTAL_ROWS))
            .map(|i| json!({"category": categories[(i % 5) as usize], "amount": i as f64 * 0.5}))
            .collect();
        client
            .tools_call("host.table.append", json!({"table": "big", "rows": rows}))
            .await
            .expect("append batch");
        start += BATCH;
    }

    let began = std::time::Instant::now();
    let result = client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM big", "handle": true}))
        .await
        .expect("materialize handle");
    let elapsed = began.elapsed();
    let summary = extract_structured(&result);

    assert!(
        elapsed.as_millis() < 2_000,
        "materialise took {}ms, expected under 2s on the builder",
        elapsed.as_millis()
    );

    assert_eq!(summary["row_count"], TOTAL_ROWS, "summary: {summary}");
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    assert!(handle.starts_with("hdl_"), "handle: {handle}");
    assert_eq!(summary["sample_cap"], 20, "summary: {summary}");
    let sample = summary["sample"].as_array().expect("sample array");
    assert_eq!(sample.len(), 20, "summary: {summary}");
    assert!(summary["bytes"].as_i64().expect("bytes") > 0, "summary: {summary}");
    assert!(summary["expires_unix"].as_i64().expect("expires_unix") > 0, "summary: {summary}");
    assert_eq!(summary["derived_from"], "SELECT * FROM big", "summary: {summary}");

    let columns = summary["columns"].as_array().expect("columns array");
    let column_names: Vec<&str> = columns.iter().map(|c| c["name"].as_str().expect("col name")).collect();
    assert!(column_names.contains(&"category"), "columns: {column_names:?}");
    assert!(column_names.contains(&"amount"), "columns: {column_names:?}");

    // The AC's own check: recompute `sum` and `distinct` by SQL over the
    // source table, and over all rows -- never the 20-row sample.
    let recomputed = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT SUM(amount) AS s, COUNT(DISTINCT category) AS d FROM big"}),
            )
            .await
            .expect("recompute over source"),
    );
    let recomputed_row = &recomputed["rows"][0];
    let expected_sum = recomputed_row["s"].as_f64().expect("sum");
    let expected_distinct = recomputed_row["d"].as_i64().expect("distinct");

    let amount_stats = &summary["stats"]["amount"];
    assert_eq!(amount_stats["distinct"], TOTAL_ROWS, "amount stats: {amount_stats}");
    let got_sum = amount_stats["sum"].as_f64().expect("amount sum");
    assert!(
        (got_sum - expected_sum).abs() < 0.01,
        "handle sum {got_sum} must match source sum {expected_sum}"
    );

    let category_stats = &summary["stats"]["category"];
    assert_eq!(
        category_stats["distinct"], expected_distinct,
        "category distinct must match source: {category_stats}"
    );
    assert!(category_stats["sum"].is_null(), "sum must be null for a non-numeric column: {category_stats}");
}
