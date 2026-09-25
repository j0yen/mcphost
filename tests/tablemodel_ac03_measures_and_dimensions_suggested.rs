//! PRD-mcphost-table-semantic-model
//! AC3 — Given the `amount` column, When `describe` runs, Then `measures`
//! suggests `sum`, `avg`, `min`, `max` for it and `dimensions` lists
//! `status` and `created`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn describe_suggests_measures_and_lists_dimensions() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "orders",
                "columns": {
                    "order_id": "integer",
                    "amount": "real",
                    "status": "text",
                    "created": "timestamp",
                },
            }),
        )
        .await
        .expect("create orders");

    let statuses = ["new", "shipped", "cancelled", "refunded"];
    let mut rows = Vec::with_capacity(200);
    for i in 0..200 {
        // index 1 repeats index 0's amount so the column is not fully
        // unique -- otherwise it would satisfy the `key` rule (checked
        // before `measure`) instead of `measure`.
        let amount_n = if i == 1 { 0 } else { i };
        rows.push(json!({
            "order_id": i,
            "amount": 10.0 + amount_n as f64 * 0.5,
            "status": statuses[i % statuses.len()],
            "created": format!("2026-02-{:02}T00:00:00Z", (i % 28) + 1),
        }));
    }
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 200 rows");

    let result = client
        .tools_call("host.table.describe", json!({"table": "orders"}))
        .await
        .expect("describe");
    let model = extract_structured(&result);

    let measures = model["measures"].as_array().expect("measures array");
    let amount_measure = measures
        .iter()
        .find(|m| m["column"] == "amount")
        .unwrap_or_else(|| panic!("expected an amount measure entry: {model}"));
    let suggestions: Vec<&str> =
        amount_measure["suggestions"].as_array().expect("suggestions array").iter().map(|v| v.as_str().unwrap()).collect();
    for expected in ["sum", "avg", "min", "max"] {
        assert!(suggestions.contains(&expected), "suggestions missing {expected}: {model}");
    }

    let dimensions: Vec<&str> = model["dimensions"].as_array().expect("dimensions array").iter().map(|v| v.as_str().unwrap()).collect();
    assert!(dimensions.contains(&"status"), "dimensions must list status: {model}");
    assert!(dimensions.contains(&"created"), "dimensions must list created: {model}");
}
