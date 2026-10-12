//! PRD-mcphost-chain-fanout-children-excludes-composition-child AC1 (P0) --
//! Given a `for_each` step over three items, When the chain runs, Then each
//! item's `runs` row has `item_index` 0, 1, 2 and the step's `step_no`, the
//! step-1 row has `item_index` null, and `host.runs.get(include_children:
//! true)` returns children carrying `role` ("step" for step 1, "item" for
//! the three), `step_no` and `item_index`, ordered step 1 then items 0, 1, 2.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::{Value, json};

/// Publishes and runs the 2-step fan-out chain; returns the parent's
/// `host.runs.get(include_children: true)` output.
async fn run_fanout_chain(client: &McpClient) -> Value {
    let schema = json!({"type": "object"});
    publish(client, "source", "echo", json!({"schema": schema})).await;
    publish(client, "worker", "echo", json!({"schema": schema})).await;
    let chain = publish(
        client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": [1, 2, 3]}},
            {"tool": "worker", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({})).await.expect("fan-out call succeeds");
    let runs = common::extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let parent_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();
    common::extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": parent_id, "include_children": true}))
            .await
            .expect("runs.get"),
    )
}

#[tokio::test]
async fn fan_out_children_carry_role_step_no_and_item_index_in_order() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfanc AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let got = run_fanout_chain(&client).await;
    let children = got["children"].as_array().expect("children array");
    let shape: Vec<(Value, Value, Value)> = children
        .iter()
        .map(|c| (c["role"].clone(), c["step_no"].clone(), c["item_index"].clone()))
        .collect();
    assert_eq!(
        shape,
        vec![
            (json!("step"), json!(1), Value::Null),
            (json!("item"), json!(2), json!(0)),
            (json!("item"), json!(2), json!(1)),
            (json!("item"), json!(2), json!(2)),
        ],
        "{got}"
    );
    let items = children.iter().filter(|c| c["role"] == json!("item")).count();
    assert_eq!(items, 3, "{got}");
    assert_eq!(children.len(), 4, "{got}");
}
