//! PRD-mcphost-chain-publish-time-path-check AC1 (P0) -- Given a tool that
//! has completed 3 runs each returning `{"items": [...], "count": n}`, When a
//! chain mapping `"rows": "$.prev.result.rows"` after it is published
//! without `dry_run`, Then the publish is refused with
//! `compose_mapping_unresolved`, `data.available` is `["count", "items"]`,
//! `data.did_you_mean` is `$.prev.result.items`, and no tool row exists.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// One async call of `tool`, polled to a terminal status (must be `done`).
async fn run_to_done(client: &McpClient, tool: &str, args: Value) {
    let enqueue = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": tool, "args": args, "async": true}))
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get ok"),
        );
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            assert_eq!(got["status"], json!("done"), "{got}");
            return;
        }
        assert!(Instant::now() < deadline, "run never finalized within 10s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_never_returned_key_is_refused_with_the_real_keys_and_no_row() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Cpath AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    for n in 1..=3 {
        run_to_done(&client, "src", json!({"items": [1, 2, n], "count": n})).await;
    }

    let spec = json!({
        "steps": [
            {"tool": "src", "args": {}},
            {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
        ]
    });
    let err = client
        .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
        .await
        .expect_err("an unresolvable mapping must be refused at publish");

    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_unresolved"), "{err:?}");
    assert_eq!(err.data["available"], json!(["count", "items"]), "{:?}", err.data);
    assert_eq!(err.data["did_you_mean"], json!("$.prev.result.items"), "{:?}", err.data);
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["path"], json!("$.prev.result.rows"), "{:?}", err.data);
    assert_eq!(err.data["prev_fields_source"], json!("observed"), "{:?}", err.data);
    assert_eq!(err.data["runs_sampled"], json!(3), "{:?}", err.data);

    // No tool row exists.
    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    let names: Vec<&str> = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(!names.contains(&format!("{ns}.pipeline").as_str()), "nothing must be published: {names:?}");
}
