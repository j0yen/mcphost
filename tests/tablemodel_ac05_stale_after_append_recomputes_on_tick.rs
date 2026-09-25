//! PRD-mcphost-table-semantic-model
//! AC5 — Given an append of 100 rows, When `describe` runs immediately,
//! Then it returns the previous model with `stale: true`; within 30s a
//! new version with the updated `row_count` exists.
//!
//! Drives `tables_model::tick_once` directly rather than waiting on the
//! real 10s background cadence -- same deterministic-tick convention
//! `bans::tick_once`/`triggers::tick_once` already use in this suite.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn append_marks_model_stale_and_tick_recomputes_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TableModel AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "orders", "columns": {"order_id": "integer", "amount": "real"}}),
        )
        .await
        .expect("create orders");
    let rows: Vec<_> = (0..500).map(|i| json!({"order_id": i, "amount": i as f64})).collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": rows}))
        .await
        .expect("append initial 500 rows");

    let first = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe (bootstrap)"),
    );
    assert_eq!(first["row_count"], 500, "model: {first}");
    assert_eq!(first["stale"], false, "a freshly bootstrapped model must not be stale: {first}");
    let first_version = first["version"].as_i64().expect("version number");

    let more_rows: Vec<_> = (500..600).map(|i| json!({"order_id": i, "amount": i as f64})).collect();
    client
        .tools_call("host.table.append", json!({"table": "orders", "rows": more_rows}))
        .await
        .expect("append 100 more rows");

    let right_after = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe (right after append)"),
    );
    assert_eq!(
        right_after["row_count"], 500,
        "describe right after an append must still return the previous model: {right_after}"
    );
    assert_eq!(right_after["stale"], true, "model: {right_after}");
    assert_eq!(right_after["version"], first_version, "model: {right_after}");

    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");

    let after_tick = extract_structured(
        &client.tools_call("host.table.describe", json!({"table": "orders"})).await.expect("describe (after tick)"),
    );
    assert_eq!(after_tick["row_count"], 600, "model: {after_tick}");
    assert_eq!(after_tick["stale"], false, "model: {after_tick}");
    assert!(
        after_tick["version"].as_i64().unwrap() > first_version,
        "recompute must bump the version: {after_tick}"
    );
}
