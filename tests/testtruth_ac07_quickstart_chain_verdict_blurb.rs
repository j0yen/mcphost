//! PRD-mcphost-tool-test-truth AC7 (P1) — Given `host.quickstart
//! kind=chain`, When it renders, Then the example maps
//! `$.prev.result.<field>` and the blurb names the `verdict` field.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, signup};
use mcphost::kinds::Kind;
use mcphost::kinds::chain::ChainKind;
use serde_json::{Value, json};

#[tokio::test]
async fn quickstart_chain_example_maps_prev_result_field() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Tool Test Truth AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = common::extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "chain"}))
            .await
            .expect("host.quickstart kind=chain must succeed"),
    );
    let publish_step = &quickstart["steps"][0];
    assert_eq!(publish_step["call"], json!("host.tool_publish"));
    let steps = publish_step["arguments"]["spec"]["steps"]
        .as_array()
        .expect("chain example spec.steps");
    assert!(steps.len() >= 2, "the example must have a second step to map $.prev into: {steps:?}");

    let mapped = steps[1]["args"]
        .as_object()
        .expect("step 2 args object")
        .values()
        .find_map(Value::as_str)
        .expect("step 2 must map at least one arg from a $.-path");

    let field = mapped
        .strip_prefix("$.prev.result.")
        .filter(|rest| !rest.is_empty());
    assert!(
        field.is_some(),
        "the example's $.prev mapping must be of the form $.prev.result.<field>, got {mapped:?}"
    );
}

#[test]
fn chain_example_blurb_names_the_verdict_field() {
    let blurb = ChainKind.example().blurb;
    assert!(
        blurb.contains("verdict"),
        "the chain kind's example blurb must name the verdict field: {blurb:?}"
    );
}
