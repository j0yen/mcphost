//! PRD-mcphost-chain-run-lineage AC6 (P0) — Given a 3-step chain whose step
//! 2 fails with `on_error: "stop"`, When awaited, Then the parent is
//! failed, child 1 is done, child 2 is failed with the step's
//! `error_class`, and no child 3 row exists.
//!
//! Step two's target is itself a (nested) chain that always refuses
//! `compose_input_missing` -- a genuine `Kind::call` failure reached only
//! AFTER `compose_call`'s own pre-dispatch checks pass (unlike, say, a bad
//! schema or a missing tool, which `compose_call` refuses before ever
//! calling into the target's own `Kind::call`), so the failure this test
//! exercises is exactly "the step's target ran and failed," not "the step
//! was never attempted." The parent's own run row keeps this crate's
//! existing call-shaped vocabulary (`done`/`error`/`timeout`) rather than
//! the literal string `"failed"` -- only a composition CHILD row uses
//! `done`/`failed` (requirement 4); the AC's "the parent is failed" reads
//! as "the parent's outcome is a failure," asserted here as `status ==
//! "error"`.

use crate::common;
use common::{chain_kind_registry, publish, signup, TestServer};
use serde_json::json;

#[tokio::test]
async fn step_two_failing_stops_the_chain_with_two_children_not_three() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "step_one", "echo", json!({"schema": schema})).await;
    // Always refuses `compose_input_missing` -- its own single step needs
    // `$.input.x`, which nothing ever supplies.
    publish(
        &client,
        "inner_broken",
        "chain",
        json!({"steps": [{"tool": "step_one", "args": {"x": "$.input.x"}}]}),
    )
    .await;

    let chain_spec = json!({
        "steps": [
            {"tool": "step_one", "args": {"n": "$.input.n"}},
            {"tool": "inner_broken", "args": {}, "on_error": "stop"},
            {"tool": "step_one", "args": {"n": "$.input.n"}},
        ]
    });
    let chain = publish(&client, "pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let err = client
        .tools_call(&chain, json!({"n": 1}))
        .await
        .expect_err("step two's failure must stop the chain");
    assert_eq!(err.error_code.as_deref(), Some("compose_input_missing"));

    let runs = common::extract_structured(
        &client
            .tools_call("host.runs.list", json!({}))
            .await
            .expect("runs.list ok"),
    );
    let parents = runs["runs"].as_array().expect("runs array");
    assert_eq!(parents.len(), 1, "one parent row: {runs}");
    let parent = &parents[0];
    assert_eq!(parent["status"], json!("error"), "the parent's outcome is a failure: {parent}");
    let parent_id = parent["run_id"].as_str().expect("parent run_id").to_string();

    let got = common::extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": parent_id}))
            .await
            .expect("runs.get ok"),
    );
    let children = got["children"].as_array().expect("children array");
    assert_eq!(children.len(), 2, "step 1 and step 2 ran; step 3 never did: {got}");
    assert_eq!(children[0]["tool"], json!("step_one"));
    assert_eq!(children[0]["status"], json!("done"));
    assert_eq!(children[1]["tool"], json!("inner_broken"));
    assert_eq!(children[1]["status"], json!("failed"));
    assert_eq!(children[1]["error_class"], json!("compose_input_missing"));
}
