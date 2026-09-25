//! PRD-mcphost-table-semantic-model
//! AC1 — Given a table with columns `order_id, customer_id, amount,
//! status, created` and 500 rows where `order_id` is unique, `status` has
//! 4 values, `amount` is numeric, `created` is ISO date, When `describe`
//! runs after refresh, Then roles are `key, id, measure, category, date`
//! respectively and `primary_key` is `order_id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn describe_infers_key_id_measure_category_date_roles() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "orders",
                "columns": {
                    "order_id": "integer",
                    "customer_id": "text",
                    "amount": "real",
                    "status": "text",
                    "created": "timestamp",
                },
            }),
        )
        .await
        .expect("create orders");

    let statuses = ["new", "shipped", "cancelled", "refunded"];
    let mut rows = Vec::with_capacity(500);
    for i in 0..500 {
        // `customer_id`: 480 distinct fixed-width values across 500 rows
        // (some repeat) -- high but not total distinct-share, so it lands
        // on the `id` rule rather than accidentally satisfying `key`.
        let customer_n = i % 480;
        // `amount`: index 1 repeats index 0's value so the column is not
        // fully unique -- otherwise it would satisfy the `key` rule
        // (checked before `measure`) instead of `measure`.
        let amount_n = if i == 1 { 0 } else { i };
        rows.push(json!({
            "order_id": i,
            "customer_id": format!("CUST-{customer_n:04}"),
            "amount": 10.0 + amount_n as f64 * 0.5,
            "status": statuses[i % statuses.len()],
            "created": format!("2026-01-{:02}T00:00:00Z", (i % 28) + 1),
        }));
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

    assert_eq!(model["row_count"], 500, "model: {model}");
    assert_eq!(model["stale"], false, "a freshly bootstrapped model must not be stale: {model}");
    assert_eq!(model["primary_key"], "order_id", "model: {model}");

    let role_of = |col: &str| -> String {
        model["columns"][col]["role"].as_str().unwrap_or_else(|| panic!("no role for {col}: {model}")).to_string()
    };
    assert_eq!(role_of("order_id"), "key");
    assert_eq!(role_of("customer_id"), "id");
    assert_eq!(role_of("amount"), "measure");
    assert_eq!(role_of("status"), "category");
    assert_eq!(role_of("created"), "date");

    // Every column also carries its own `inferred_role`, unaffected by
    // (as-yet-nonexistent) annotations -- AC8 later proves an annotation
    // can diverge `role` from it.
    for col in ["order_id", "customer_id", "amount", "status", "created"] {
        assert_eq!(model["columns"][col]["inferred_role"], Value::String(role_of(col)), "model: {model}");
    }
}
