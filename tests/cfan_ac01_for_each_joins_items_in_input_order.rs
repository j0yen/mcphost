//! PRD-mcphost-chain-fanout-and-join AC1 (P0) -- Given a 2-step chain whose
//! step 1 returns `{"items": [1, 2, 3]}` and step 2 declares `for_each:
//! "$.prev.result.items"` with `args: {"n": "$.item"}`, When called, Then
//! step 2 runs 3 times, the chain result is `{"items": [r1, r2, r3],
//! "failed": [], "ok": 3, "failed_count": 0}` in input order, and
//! `host.runs.list(parent_run_id)` returns 3 child rows with `trigger_ref`
//! ending `#2:0`, `#2:1`, `#2:2`.

use crate::common;
use common::{TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_for_each_step_runs_per_item_and_joins_in_input_order() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "source", "echo", json!({"schema": schema})).await;
    publish(&client, "worker", "echo", json!({"schema": schema})).await;
    let chain = publish(
        &client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": "$.input.items"}},
            {"tool": "worker", "args": {"n": "$.item", "at": "$.index"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;

    let call = client.tools_call(&chain, json!({"items": [1, 2, 3]})).await.expect("fan-out call succeeds");
    let structured = common::extract_structured(&call);
    assert_eq!(structured["failed_steps"], json!([]), "{structured}");
    let result = &structured["result"];
    assert_eq!(result["failed"], json!([]), "{result}");
    assert_eq!(result["ok"], json!(3), "{result}");
    assert_eq!(result["failed_count"], json!(0), "{result}");
    let items = result["items"].as_array().expect("items array");
    assert_eq!(items.len(), 3, "{result}");
    // Input order: each slot echoes its own item and index.
    let text = serde_json::to_string(items).expect("serialize");
    for (i, n) in [1, 2, 3].iter().enumerate() {
        let slot = serde_json::to_string(&items[i]).expect("serialize slot");
        assert!(slot.contains(&format!("\"n\":{n}")) && slot.contains(&format!("\"at\":{i}")), "slot {i}: {text}");
    }

    let runs = common::extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let parent_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();
    let got = common::extract_structured(
        &client.tools_call("host.runs.get", json!({"run_id": parent_id})).await.expect("runs.get"),
    );
    let children = got["children"].as_array().expect("children array");
    let mut refs: Vec<String> = children
        .iter()
        .filter(|c| c["tool"] == json!("worker"))
        .map(|c| c["trigger_ref"].as_str().expect("trigger_ref").to_string())
        .collect();
    refs.sort();
    assert_eq!(refs.len(), 3, "three item rows: {got}");
    for (i, r) in refs.iter().enumerate() {
        assert_eq!(r, &format!("{parent_id}#2:{i}"), "{refs:?}");
    }
}
