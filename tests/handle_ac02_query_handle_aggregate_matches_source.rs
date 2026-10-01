//! PRD-mcphost-result-handles
//! AC2 -- Given that handle, When `SELECT category, SUM(amount) FROM
//! hdl_<id> GROUP BY category` runs through `host.table.query`, Then the
//! rows match the same aggregate over the source table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn aggregate_map(rows: &[Value]) -> BTreeMap<String, f64> {
    rows.iter()
        .map(|r| {
            (
                r["category"].as_str().expect("category").to_string(),
                r["total"].as_f64().expect("total"),
            )
        })
        .collect()
}

#[tokio::test]
async fn query_over_handle_group_by_matches_source_aggregate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    let categories = ["a", "b", "c"];
    let rows: Vec<_> = (0..300)
        .map(|i| json!({"category": categories[i % 3], "amount": (i as f64) + 0.25}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append");

    let materialize = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM orders", "handle": true}))
            .await
            .expect("materialize"),
    );
    let handle = materialize["handle"].as_str().expect("handle").to_string();

    let from_handle = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": format!("SELECT category, SUM(amount) AS total FROM {handle} GROUP BY category ORDER BY category")}),
            )
            .await
            .expect("query handle"),
    );
    let from_source = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT category, SUM(amount) AS total FROM orders GROUP BY category ORDER BY category"}),
            )
            .await
            .expect("query source"),
    );

    let handle_rows = from_handle["rows"].as_array().expect("rows");
    let source_rows = from_source["rows"].as_array().expect("rows");
    assert_eq!(handle_rows.len(), 3, "rows: {handle_rows:?}");
    assert_eq!(aggregate_map(handle_rows), aggregate_map(source_rows));
}
