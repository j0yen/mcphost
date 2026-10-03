//! PRD-mcphost-drift-review
//! AC9 -- Given 50 affected queries on the fixture, When a change is
//! processed, Then re-runs finish under 5s on the builder and the
//! tenant's daily call count rose by one.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn fifty_affected_queries_rerun_fast_and_meter_one_call() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Drift AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find tenant").expect("tenant exists");

    client
        .tools_call("host.table.create", json!({"name": "events", "columns": {"n": "integer"}}))
        .await
        .expect("create events");
    let rows: Vec<_> = (0..100).map(|i| json!({"n": i})).collect();
    client
        .tools_call("host.table.append", json!({"table": "events", "rows": rows}))
        .await
        .expect("append");

    for i in 0..50 {
        client
            .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM events WHERE n > {i}")}))
            .await
            .unwrap_or_else(|e| panic!("query {i} failed: {} {}", e.code, e.message));
    }

    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    let calls_before = server.state.db.count_calls_since(tenant.id, midnight, true).await.expect("count_calls_since");

    client
        .tools_call(
            "host.table.model_set",
            json!({"table": "events", "key": "description", "value": "event counter"}),
        )
        .await
        .expect("model_set");

    let started = Instant::now();
    mcphost::tables_model::tick_once(&server.state).await.expect("tick_once");
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(5), "re-run took too long: {elapsed:?}");

    let calls_after = server.state.db.count_calls_since(tenant.id, midnight, true).await.expect("count_calls_since");
    assert_eq!(calls_after, calls_before + 1, "daily call count must rise by exactly one: before={calls_before} after={calls_after}");
}
