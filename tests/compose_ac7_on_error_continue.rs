//! PRD-mcphost-composition AC7 (P0) — Given a chain with `on_error:
//! "continue"` on step two, When step two fails, Then step three runs and
//! the parent ends `done` with `failed_steps: [2]`.

mod common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn step_two_continuing_past_failure_lets_step_three_run_and_the_chain_succeed() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "On Error Continue Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step3", "echo", json!({"schema": schema})).await;
    // "step2" is deliberately never published -- its dispatch fails with
    // `tool_not_found`, exercising the `on_error: "continue"` path without
    // needing a second failure mode.

    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            {"tool": "step2", "args": {}, "on_error": "continue"},
            // Step three doesn't depend on step two's (missing) result, so
            // it can succeed regardless.
            {"tool": "step3", "args": {"k": "$.input.n"}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let call = client
        .tools_call(&chain, json!({"n": 7}))
        .await
        .expect("the chain must still succeed overall");
    let structured = common::extract_structured(&call);

    assert_eq!(structured["result"], json!({"k": 7}));
    assert_eq!(structured["failed_steps"], json!([2]));

    let steps = structured["steps"].as_array().expect("steps trace");
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0]["status"], json!("done"));
    assert_eq!(steps[1]["status"], json!("error"));
    assert_eq!(steps[1]["error_class"], json!("tool_not_found"));
    assert_eq!(steps[2]["status"], json!("done"));
}
