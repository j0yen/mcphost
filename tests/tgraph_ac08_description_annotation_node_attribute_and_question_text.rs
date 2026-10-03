//! PRD-mcphost-table-concept-graph
//! AC8 — Given `host.table.model_set("orders", column "amount", key
//! "description", value "order total in USD")`, When the next rebuild
//! runs, Then the `amount` node carries that text as an attribute and
//! `next_questions` uses "order total in USD" in the matching question
//! text.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn model_set_description_lands_on_graph_node_and_next_questions_text() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TGraph AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "customers", "columns": {"id": "text", "name": "text"}}),
        )
        .await
        .expect("create customers");
    let customer_ids: Vec<String> = (0..20).map(|i| format!("CUST-{i:04}")).collect();
    let customer_rows: Vec<_> =
        customer_ids.iter().map(|id| json!({"id": id, "name": format!("Customer {id}")})).collect();
    client
        .tools_call("host.table.append", json!({"table": "customers", "rows": customer_rows}))
        .await
        .expect("append customers");

    client
        .tools_call(
            "host.table.create",
            json!({
                "name": "orders",
                "columns": {"id": "text", "customer_id": "text", "amount": "real", "day": "text"},
            }),
        )
        .await
        .expect("create orders");
    let order_rows: Vec<_> = (0..100)
        .map(|i| {
            json!({
                "id": format!("ORD-{i:04}"),
                "customer_id": customer_ids[i % customer_ids.len()],
                "amount": 5.0 + (i % 43) as f64,
                "day": format!("2026-03-{:02}", 1 + (i % 25)),
            })
        })
        .collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": order_rows})).await.expect("append orders");

    client.tools_call("host.table.describe", json!({"table": "customers"})).await.expect("describe customers");
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe orders");

    // Bootstraps the graph before the annotation exists -- requirement 7's
    // own claim is that a *later* model_set still reaches an
    // already-built graph via the next rebuild, not only a fresh bootstrap.
    let before = extract_structured(
        &client.tools_call("host.table.graph", json!({"table": "orders", "hops": 1})).await.expect("graph (before)"),
    );
    let amount_before = before["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .find(|n| n["id"] == "orders.amount")
        .expect("orders.amount node");
    assert!(
        amount_before["attributes"].get("description").is_none(),
        "no description should be set yet: {amount_before}"
    );

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "orders", "column": "amount", "key": "description", "value": "order total in USD"}),
        )
        .await
        .expect("model_set description");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let after = extract_structured(
        &client.tools_call("host.table.graph", json!({"table": "orders", "hops": 1})).await.expect("graph (after)"),
    );
    let amount_after = after["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .find(|n| n["id"] == "orders.amount")
        .expect("orders.amount node");
    assert_eq!(
        amount_after["attributes"]["description"], json!("order total in USD"),
        "amount node must carry the description attribute: {amount_after}"
    );

    let questions = extract_structured(
        &client.tools_call("host.table.next_questions", json!({"table": "orders"})).await.expect("next_questions"),
    );
    let matched = questions["questions"]
        .as_array()
        .expect("questions array")
        .iter()
        .any(|q| q["question"].as_str().expect("question string").contains("order total in USD"));
    assert!(matched, "expected a next_questions entry using the description text: {questions}");
}
