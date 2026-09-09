//! PRD-mcphost-composition AC5 (P0) — Given a `chain` spec of three steps
//! with `$.prev` mappings, When called, Then a parent run and three child
//! runs exist in order and the result is step three's result.
//!
//! No `runs` table exists yet (PRD-mcphost-runs-and-jobs hasn't shipped),
//! so "three child runs exist in order" is checked against this kind's own
//! `steps` trace (the documented stand-in -- see `kinds::chain`'s module
//! doc) rather than `host.runs.list`; "the result is step three's result"
//! is checked directly.

mod common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn three_steps_run_in_order_via_prev_and_steps_mappings_result_is_step_threes() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Chain Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Every step is `echo` (permissive schema): it validates and returns
    // its own arguments verbatim, so the composed result at each hop is
    // exactly the mapping this test can check.
    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;
    publish(&client, "step3", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step1", "args": {"n": "$.input.n"}},
            {"tool": "step2", "args": {"m": "$.prev.result.n"}},
            {"tool": "step3", "args": {"k": "$.steps[1].result.m"}}
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let call = client
        .tools_call(&chain, json!({"n": 5}))
        .await
        .expect("chain call should succeed");
    let structured = common::extract_structured(&call);

    assert_eq!(
        structured["result"],
        json!({"k": 5}),
        "the chain's result must be step three's result"
    );
    assert!(structured["failed_steps"].as_array().unwrap().is_empty());

    let steps = structured["steps"].as_array().expect("steps trace");
    assert_eq!(steps.len(), 3, "three child dispatches, in order");
    let tools: Vec<&str> = steps.iter().map(|s| s["tool"].as_str().unwrap()).collect();
    assert_eq!(tools, vec!["step1", "step2", "step3"]);
    for s in steps {
        assert_eq!(s["status"], json!("done"));
    }
}
