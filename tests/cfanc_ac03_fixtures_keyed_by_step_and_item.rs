//! PRD-mcphost-chain-fanout-children-excludes-composition-child AC3 (P0) --
//! Given a 2-step chain whose step 2 fans out over three items, When
//! `child_fixtures` builds `host.tool_test` fixtures, Then the fixtures
//! object has keys "1", "2:0", "2:1", "2:2" with each item's own result and
//! an empty `truncated`; Given one purged item, Then `truncated` names "2:1"
//! and the other keys are present.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::{Value, json};

async fn fixtures_of(client: &McpClient, parent_id: &str) -> Value {
    common::extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": parent_id, "as_fixtures": true, "include_children": true}))
            .await
            .expect("runs.get as_fixtures"),
    )
}

#[tokio::test]
async fn fixtures_hold_every_item_and_name_a_purged_one_in_truncated() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfanc AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "source", "echo", json!({"schema": schema})).await;
    publish(&client, "worker", "echo", json!({"schema": schema})).await;
    let chain = publish(
        &client,
        "fan",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": [10, 20, 30]}},
            {"tool": "worker", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;
    client.tools_call(&chain, json!({})).await.expect("fan-out call succeeds");
    let runs = common::extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let parent_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();

    let got = fixtures_of(&client, &parent_id).await;
    let fixtures = got["fixtures"].as_object().expect("fixtures object");
    assert_eq!(fixtures.keys().cloned().collect::<Vec<_>>(), ["1", "2:0", "2:1", "2:2"], "{got}");
    assert_eq!(fixtures["1"]["items"], json!([10, 20, 30]), "{got}");
    for (i, n) in [10, 20, 30].iter().enumerate() {
        assert_eq!(fixtures[&format!("2:{i}")]["n"], json!(n), "item {i}: {got}");
    }
    assert_eq!(got["truncated"], json!([]), "{got}");

    // Purge exactly item 1's stored result (its `runs/<id>/part/0` key).
    let item_1 = got["children"]
        .as_array()
        .expect("children")
        .iter()
        .find(|c| c["item_index"] == json!(1))
        .expect("item 1 child")["run_id"]
        .as_str()
        .expect("item run id")
        .to_string();
    client
        .tools_call("host.state.delete", json!({"key": format!("runs/{item_1}/part/0")}))
        .await
        .expect("purge one item's stored result");

    let got = fixtures_of(&client, &parent_id).await;
    let fixtures = got["fixtures"].as_object().expect("fixtures object");
    assert_eq!(fixtures.keys().cloned().collect::<Vec<_>>(), ["1", "2:0", "2:2"], "{got}");
    assert_eq!(got["truncated"], json!(["2:1"]), "{got}");
}
