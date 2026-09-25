//! PRD-mcphost-table-semantic-model
//! AC4 — Given 20% nulls in `status`, When `describe` runs, Then
//! `null_share` is 0.2 ± 0.01 and the column is still `category`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn describe_reports_null_share_and_keeps_category_role() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer", "status": "text"}}),
        )
        .await
        .expect("create orders");

    let statuses = ["new", "shipped", "cancelled", "refunded"];
    let mut rows = Vec::with_capacity(500);
    for i in 0..500 {
        // Every 5th row (100 of 500 = 20%) omits `status` entirely, which
        // `host.table.append` leaves NULL in that column -- a JSON `null`
        // value would instead fail schema validation (`status` is
        // declared `text`, and `null` doesn't match `Value::is_string`).
        if i % 5 == 0 {
            rows.push(json!({"order_id": i}));
        } else {
            rows.push(json!({"order_id": i, "status": statuses[i % statuses.len()]}));
        }
    }
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 500 rows");

    let result = client
        .tools_call("host.table.describe", json!({"table": "orders"}))
        .await
        .expect("describe");
    let model = extract_structured(&result);

    let null_share = model["columns"]["status"]["null_share"].as_f64().expect("null_share number");
    assert!(
        (null_share - 0.2).abs() <= 0.01,
        "expected null_share ~0.2, got {null_share}: {model}"
    );
    assert_eq!(model["columns"]["status"]["role"], "category", "model: {model}");
}
