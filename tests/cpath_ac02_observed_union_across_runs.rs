//! PRD-mcphost-chain-publish-time-path-check AC2 (P0) -- Given a tool whose
//! runs returned `{a, b}` then `{a, c}`, When a chain mapping
//! `$.prev.result.c` is published, Then it succeeds and the response carries
//! `prev_fields_source: "observed"`, `runs_sampled: 2`, and
//! `prev_fields: ["a", "b", "c"]`.

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

async fn setup(name: &str) -> (TestServer, McpClient, String) {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    (server, client, ns)
}

#[tokio::test]
async fn a_key_seen_in_any_run_resolves_and_the_response_reports_the_sample() {
    let (_server, client, _ns) = setup("Cpath AC2 Tenant").await;
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    run_to_done(&client, "src", json!({"a": 1, "b": 2})).await;
    run_to_done(&client, "src", json!({"a": 1, "c": 3})).await;

    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"v": "$.prev.result.c"}},
    ]});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
            .await
            .expect("a key observed in one of two runs must publish"),
    );

    assert_eq!(published["prev_fields_source"], json!("observed"), "{published}");
    assert_eq!(published["runs_sampled"], json!(2), "{published}");
    assert_eq!(published["prev_fields"], json!(["a", "b", "c"]), "{published}");
    assert_eq!(published["verdict"], json!("pass"), "{published}");
    // The same facts ride on the per-step entry.
    assert_eq!(published["steps"][0]["prev_fields_source"], json!("observed"), "{published}");
}
