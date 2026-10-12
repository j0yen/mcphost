//! PRD-mcphost-chain-fanout-and-join AC4 (P0) -- Given 21 items on a free
//! plan, When called, Then the call is refused with `compose_fanout_exceeded`,
//! `data: {limit: 20, actual: 21}`, and no child run exists.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn twenty_one_items_on_free_are_refused_with_no_child_run() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    publish(&client, "source", "echo", json!({"schema": schema})).await;
    publish(&client, "worker", "echo", json!({"schema": schema})).await;
    let chain = publish(
        &client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": "$.input.items"}},
            {"tool": "worker", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;

    let at_ceiling: Vec<u32> = (0..20).collect();
    client.tools_call(&chain, json!({"items": at_ceiling})).await.expect("20 items is within the ceiling");

    let over: Vec<u32> = (0..21).collect();
    let err = client.tools_call(&chain, json!({"items": over})).await.expect_err("21 items must be refused");
    assert_eq!(err.error_code.as_deref(), Some("compose_fanout_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!(20), "{:?}", err.data);
    assert_eq!(err.data["actual"], json!(21), "{:?}", err.data);

    // The refused call is the newest parent; only the source step (a plain
    // step 1) ran under it -- no `worker` child exists for it.
    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let refused = runs["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .find(|r| r["status"] == json!("error"))
        .expect("the refused call's own run row");
    let got = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": refused["run_id"]}))
            .await
            .expect("runs.get"),
    );
    let workers = got["children"]
        .as_array()
        .expect("children array")
        .iter()
        .filter(|c| c["tool"] == json!("worker"))
        .count();
    assert_eq!(workers, 0, "no item child run exists for the refused call: {got}");
}
