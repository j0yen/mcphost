//! PRD-mcphost-run-budget-governor
//! AC3 (P0) — Given a free-plan tenant passing `max_child_calls: 100`, When
//! `host.tool_call` is invoked, Then a validation error names the plan
//! ceiling 20 and no run starts.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn budget_above_the_free_plan_ceiling_is_refused_before_any_run_starts() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    // Synchronous path: the same validation must reject before
    // call_published_tool ever dispatches.
    let sync_err = client
        .tools_call(
            "host.tool_call",
            json!({"name": "echoer", "args": {}, "budget": {"max_child_calls": 100}}),
        )
        .await
        .expect_err("a budget above the free plan's ceiling must be refused");
    assert_eq!(sync_err.error_code.as_deref(), Some("budget_ceiling_exceeded"));
    let data = &sync_err.data;
    assert_eq!(data["ceiling"], json!(20), "must name the free plan's own ceiling: {data}");
    assert_eq!(data["requested"], json!(100));
    assert_eq!(data["field"], json!("budget.max_child_calls"));

    // Async path: the same validation must reject before any run is
    // enqueued -- no run_id, no queued row.
    let async_err = client
        .tools_call(
            "host.tool_call",
            json!({
                "name": "echoer",
                "args": {},
                "async": true,
                "budget": {"max_child_calls": 100},
            }),
        )
        .await
        .expect_err("async dispatch must be refused the same way");
    assert_eq!(async_err.error_code.as_deref(), Some("budget_ceiling_exceeded"));

    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    assert_eq!(
        runs["runs"].as_array().expect("runs array").len(),
        0,
        "no run must exist for tenant {ns}: {runs}"
    );
}
