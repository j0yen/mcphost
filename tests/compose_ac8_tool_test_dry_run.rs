//! PRD-mcphost-composition AC8 (P0) — Given `host.tool_test` on a chain,
//! When run, Then the report lists each step's resolved arguments and no
//! step executes.
//!
//! Neither step's target tool is ever published; if the dry run actually
//! dispatched a step, `compose_call` would fail it with `tool_not_found`
//! and `host.tool_test` would report that failure instead of succeeding --
//! this call succeeding at all is itself proof no step executed.

mod common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn tool_test_on_a_chain_reports_resolved_args_per_step_without_executing_any() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let chain_spec = json!({
        "steps": [
            {"tool": "does_not_exist_1", "args": {"x": "$.input.x"}},
            {"tool": "does_not_exist_2", "args": {"y": "$.prev.result.y"}}
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pipeline", "args": {"x": 42}}),
        )
        .await
        .expect("tool_test must succeed -- it must never dispatch a step");
    let structured = common::extract_structured(&result);

    assert_eq!(structured["dry_run"], json!(true));
    let steps = structured["steps"].as_array().expect("steps report");
    assert_eq!(steps.len(), 2);

    assert_eq!(steps[0]["tool"], json!("does_not_exist_1"));
    assert_eq!(
        steps[0]["resolved_args"]["x"],
        json!(42),
        "$.input.x resolves against the real test call args"
    );

    assert_eq!(steps[1]["tool"], json!("does_not_exist_2"));
    assert_eq!(
        steps[1]["resolved_args"]["y"]["unresolved_path"],
        json!("$.prev.result.y"),
        "a $.prev mapping can't resolve for real -- no step has run"
    );
}
