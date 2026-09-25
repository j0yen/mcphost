//! PRD-mcphost-table-semantic-model
//! AC7 — Given `model_set {table: "orders", column: "amount", key:
//! "unit", value: "USD"}` and a later refresh, When `describe` runs, Then
//! `amount.unit` is `USD` and inferred fields are updated.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn model_set_unit_annotation_survives_a_refresh() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    let rows: Vec<_> = (0..100).map(|i| json!({"order_id": i, "amount": i as f64})).collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append 100 rows");

    // Bootstrap a model to annotate.
    client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("bootstrap describe");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "orders", "column": "amount", "key": "unit", "value": "USD"}),
        )
        .await
        .expect("model_set unit");

    let before_refresh = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe before refresh"),
    );
    assert_eq!(before_refresh["columns"]["amount"]["unit"], "USD", "model: {before_refresh}");

    // A later refresh (append + tick) must not drop the annotation, and
    // inferred fields (row_count) must still update.
    let more_rows: Vec<_> = (100..150).map(|i| json!({"order_id": i, "amount": i as f64})).collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": more_rows}))
        .await
        .expect("append 50 more rows");
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let after_refresh = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe after refresh"),
    );
    assert_eq!(after_refresh["columns"]["amount"]["unit"], "USD", "model: {after_refresh}");
    assert_eq!(after_refresh["row_count"], 150, "inferred row_count must update: {after_refresh}");
    assert_eq!(after_refresh["stale"], false, "model: {after_refresh}");
}
