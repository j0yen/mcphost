//! PRD-mcphost-table-semantic-model
//! AC6 — Given 50 000 rows, When the model computes, Then it samples
//! 10 000, reports `sampled: true`, and completes in < 500 ms.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn describe_samples_10000_of_50000_rows_within_500ms() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TableModel AC6 Tenant").await;
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
            json!({"name": "big", "columns": {"id": "integer", "value": "real"}}),
        )
        .await
        .expect("create big");

    // Batched (2 MiB request body cap): 10 appends of 5,000 rows each.
    const TOTAL_ROWS: i64 = 50_000;
    const BATCH: i64 = 5_000;
    let mut start = 0;
    while start < TOTAL_ROWS {
        let rows: Vec<_> = (start..(start + BATCH).min(TOTAL_ROWS))
            .map(|i| json!({"id": i, "value": i as f64 * 0.5}))
            .collect();
        client
            .tools_call("host.table.append", json!({"table": "big", "rows": rows}))
            .await
            .expect("append batch");
        start += BATCH;
    }

    let began = std::time::Instant::now();
    let result = client
        .tools_call("host.table.describe", json!({"table": "big"}))
        .await
        .expect("describe (bootstrap compute)");
    let elapsed = began.elapsed();
    let model = extract_structured(&result);

    assert_eq!(model["row_count"], TOTAL_ROWS, "model: {model}");
    assert_eq!(model["sample_count"], 10_000, "model: {model}");
    assert_eq!(model["sampled"], true, "model: {model}");
    assert!(
        elapsed.as_millis() < 500,
        "describe's compute took {}ms, expected < 500ms",
        elapsed.as_millis()
    );
}
