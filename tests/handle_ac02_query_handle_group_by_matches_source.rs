//! PRD-mcphost-result-handles
//! AC2 -- Given that handle, When `SELECT category, SUM(amount) FROM
//! hdl_<id> GROUP BY category` runs through `host.table.query`, Then the
//! rows match the same aggregate over the source table.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn as_group_map(rows: &[Value]) -> BTreeMap<String, f64> {
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
async fn group_by_over_handle_matches_group_by_over_source() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC2 Tenant").await;
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
        .map(|i| {
            let category = categories[(i % 3) as usize];
            json!({
                "category": category,
                "amount": (i as f64) * 1.5,
            })
        })
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append orders");

    let summary = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM orders", "handle": true}))
            .await
            .expect("materialize handle"),
    );
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    assert_eq!(summary["row_count"], 300, "summary: {summary}");

    let over_handle = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": format!(
                    "SELECT category, SUM(amount) AS total FROM {handle} GROUP BY category ORDER BY category"
                )}),
            )
            .await
            .expect("group by over handle"),
    );
    let over_source = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT category, SUM(amount) AS total FROM orders GROUP BY category ORDER BY category"}),
            )
            .await
            .expect("group by over source"),
    );

    let handle_rows = over_handle["rows"].as_array().expect("rows array");
    let source_rows = over_source["rows"].as_array().expect("rows array");
    assert_eq!(handle_rows.len(), 3, "rows: {handle_rows:?}");
    assert_eq!(
        as_group_map(handle_rows),
        as_group_map(source_rows),
        "GROUP BY over the handle must match the same aggregate over the source table"
    );

    // requirement 4's eviction order: querying the handle must bump its
    // last_used_unix -- visible through host.table.handles.
    let handles = extract_structured(
        &client.tools_call("host.table.handles", json!({})).await.expect("host.table.handles"),
    );
    let listed = handles["handles"].as_array().expect("handles array");
    let entry = listed
        .iter()
        .find(|h| h["handle"] == json!(handle))
        .expect("the materialised handle must be listed");
    assert!(entry["last_used_unix"].as_i64().unwrap() >= entry["created_unix"].as_i64().unwrap());
}
