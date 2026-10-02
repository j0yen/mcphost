//! PRD-mcphost-chain-run-lineage AC8 (P0) — Given one 3-step chain run,
//! When `host.usage` and the `calls` ledger are read, Then the chain
//! counts as exactly 1 call and the `busyaudit`/`metering` suites pass
//! unchanged.
//!
//! The `busyaudit`/`metering` suites themselves (`tests/busyaudit_ac*.rs`,
//! `tests/metering_ac*.rs`) are untouched by this PRD and run as part of
//! the same `cargo test` invocation as this file -- this test only proves
//! the new half: a composed chain's THREE step dispatches still meter as
//! ONE `calls` row (the parent's), not four.

use crate::common;
use common::{chain_kind_registry, extract_structured, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn a_three_step_chain_call_meters_as_exactly_one_call() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    client
        .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
        .await
        .expect("call ok");

    let usage = extract_structured(
        &client
            .tools_call("host.usage", json!({}))
            .await
            .expect("usage ok"),
    );
    assert_eq!(usage["calls"], json!(1), "a 3-step chain call must meter as 1 call: {usage}");

    // Confirm the ledger itself, not just `host.usage`'s own derivation of
    // it: exactly one row in `calls`, found via the tenant directly.
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let calls_count = server
        .state
        .db
        .count_calls_since(tenant.id, 0, false)
        .await
        .expect("count_calls_since");
    assert_eq!(calls_count, 1, "the calls ledger itself must have exactly one row");
}
