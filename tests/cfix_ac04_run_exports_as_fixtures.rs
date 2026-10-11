//! PRD-mcphost-chain-dry-run AC4 (P0) -- Given a completed chain run with 3
//! child rows, When `host.runs.get(run_id, as_fixtures=true)` is called and
//! its `fixtures` passed to `host.tool_test` of the same chain, Then the dry
//! run is `pass` with `fixtured: 3`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn a_real_runs_children_replay_as_fixtures() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Cfix AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    for name in ["hop_a", "hop_b", "hop_c"] {
        publish(&client, name, "echo", json!({"schema": schema})).await;
    }
    let spec = json!({"steps": [
        {"tool": "hop_a", "args": {"items": [1, 2]}},
        {"tool": "hop_b", "args": {"items": "$.prev.result.items"}},
        {"tool": "hop_c", "args": {"items": "$.prev.result.items"}},
    ]});
    let chain = publish(&client, "pipeline", "chain", spec).await;
    assert_eq!(chain, format!("{ns}.pipeline"));
    client.tools_call(&chain, json!({})).await.expect("the real chain run");

    let runs = common::extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("runs.list"));
    let run_id = runs["runs"][0]["run_id"].as_str().expect("parent run id").to_string();

    let got = common::extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id, "as_fixtures": true}))
            .await
            .expect("runs.get as_fixtures"),
    );
    let fixtures = got["fixtures"].as_object().expect("fixtures object");
    assert_eq!(fixtures.keys().cloned().collect::<Vec<_>>(), ["1", "2", "3"], "{got}");
    assert_eq!(fixtures["1"]["items"], json!([1, 2]), "{got}");
    assert_eq!(got["truncated"], json!([]), "{got}");

    // Without the flag the field is absent (additive argument).
    let plain = common::extract_structured(
        &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"),
    );
    assert!(plain.get("fixtures").is_none(), "{plain}");

    let dry = common::extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                json!({"name": "pipeline", "args": {}, "fixtures": got["fixtures"]}),
            )
            .await
            .expect("dry run over the exported fixtures"),
    );
    assert_eq!(dry["verdict"], json!("pass"), "{dry}");
    assert_eq!(dry["fixtured"], json!(3), "{dry}");
    assert_eq!(dry["executed"], json!(0), "{dry}");
}
