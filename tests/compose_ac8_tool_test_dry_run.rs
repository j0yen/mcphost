//! PRD-mcphost-composition AC8 (P0) — Given `host.tool_test` on a chain,
//! When run, Then the report lists each step's resolved arguments and no
//! step executes.
//!
//! Neither step's target tool exists BY THE TIME the dry run runs; if it
//! actually dispatched a step, `compose_call` would fail it with
//! `tool_not_found` and `host.tool_test` would report that failure instead
//! of succeeding -- this call succeeding at all is itself proof no step
//! executed.
//!
//! PRD-mcphost-chain-host-steps requirement 4 (landed after this PRD):
//! `host.tool_publish` now resolves every step's tool before publishing, so
//! both step targets are published first (as trivial `echo` tools) purely
//! to satisfy that resolution, then removed again before this test ever
//! calls `host.tool_test` -- "does not exist" by the time the dry run
//! matters, same as before this PRD.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn tool_test_on_a_chain_reports_resolved_args_per_step_without_executing_any() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "does_not_exist_1", "echo", json!({"schema": schema.clone()})).await;
    publish(&client, "does_not_exist_2", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "does_not_exist_1", "args": {"x": "$.input.x"}},
            {"tool": "does_not_exist_2", "args": {"y": "$.prev.result.y"}}
        ]
    });
    publish(&client, "pipeline", "chain", chain_spec).await;

    client
        .tools_call("host.tool_remove", json!({"name": "does_not_exist_1"}))
        .await
        .expect("host.tool_remove must succeed");
    client
        .tools_call("host.tool_remove", json!({"name": "does_not_exist_2"}))
        .await
        .expect("host.tool_remove must succeed");

    let result = client
        .tools_call(
            "host.tool_test",
            json!({"name": "pipeline", "args": {"x": 42}}),
        )
        .await
        .expect("tool_test must succeed -- it must never dispatch a step");
    let structured = common::extract_structured(&result);

    // PRD-mcphost-dry-run-side-effects: `dry_run` is now the shared result
    // envelope's own `{writes, delivered, rolled_back}` object (every
    // `host.tool_test` call gets one) -- `chain`'s own pre-PRD marker was
    // renamed to `chain_dry_run` to stop the two colliding.
    assert_eq!(structured["chain_dry_run"], json!(true));
    assert_eq!(structured["dry_run"]["writes"], json!([]));
    assert_eq!(structured["dry_run"]["rolled_back"], json!(true));
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
