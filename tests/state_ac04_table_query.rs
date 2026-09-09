//! PRD-mcphost-tenant-state
//! AC4 — Given `host.state.table_create(name="alerts", schema={"metric":
//! "text", "last_value": "real", "acked": "boolean"}, primary_key="metric")`,
//! When three rows are inserted and `host.state.query(table="alerts",
//! where="last_value > 0.5", order_by="last_value desc", limit=2)` runs,
//! Then exactly the two matching rows return in order.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn query_filters_orders_and_limits() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Monitor").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "alerts",
                "schema": {"metric": "text", "last_value": "real", "acked": "boolean"},
                "primary_key": "metric",
            }),
        )
        .await
        .expect("table_create");

    client
        .tools_call(
            "host.state.insert",
            json!({"table": "alerts", "rows": [
                {"metric": "a", "last_value": 0.9, "acked": false},
                {"metric": "b", "last_value": 0.2, "acked": false},
                {"metric": "c", "last_value": 0.7, "acked": true},
            ]}),
        )
        .await
        .expect("insert");

    let result = extract_structured(
        &client
            .tools_call(
                "host.state.query",
                json!({"table": "alerts", "where": "last_value > 0.5", "order_by": "last_value desc", "limit": 2}),
            )
            .await
            .expect("query"),
    );
    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["metric"], json!("a"));
    assert_eq!(rows[1]["metric"], json!("c"));
}

#[tokio::test]
async fn insert_upserts_on_primary_key() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Monitor").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.state.table_create",
            json!({
                "name": "alerts",
                "schema": {"metric": "text", "last_value": "real"},
                "primary_key": "metric",
            }),
        )
        .await
        .expect("table_create");

    client
        .tools_call(
            "host.state.insert",
            json!({"table": "alerts", "rows": [{"metric": "cpu", "last_value": 0.1}]}),
        )
        .await
        .expect("first insert");
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "alerts", "rows": [{"metric": "cpu", "last_value": 0.9}]}),
        )
        .await
        .expect("upserting insert");

    let result = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "alerts"}))
            .await
            .expect("query"),
    );
    let rows = result["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "a same-primary-key insert must replace, not accumulate: {rows:?}");
    assert_eq!(rows[0]["last_value"], json!(0.9));
}
