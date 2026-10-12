//! PRD-mcphost-chain-publish-time-path-check AC5 (P0) -- Given a 3-step
//! chain whose step 3 maps `$.steps[0].result.count` and step 1 has observed
//! keys `{items, count}`, When published, Then it passes; and Given step 2
//! maps `$.steps[2].result.x`, Then `will_fail` with `reason: "forward
//! reference"`.

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

async fn setup_observed() -> (TestServer, McpClient) {
    let (server, client, _ns) = setup("Cpath AC5 Tenant").await;
    for name in ["src", "mid", "sink"] {
        publish(&client, name, "echo", json!({"schema": {"type": "object"}})).await;
    }
    run_to_done(&client, "src", json!({"items": [1], "count": 1})).await;
    (server, client)
}

#[tokio::test]
async fn a_steps_index_reads_that_steps_observed_keys() {
    let (_server, client) = setup_observed().await;
    let ok = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "mid", "args": {}},
        {"tool": "sink", "args": {"n": "$.steps[0].result.count"}},
    ]});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "ok_chain", "kind": "chain", "spec": ok}))
            .await
            .expect("count was observed on step 1"),
    );
    assert_eq!(published["verdict"], json!("pass"), "{published}");

    let missing = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "mid", "args": {}},
        {"tool": "sink", "args": {"n": "$.steps[0].result.total"}},
    ]});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "bad_chain", "kind": "chain", "spec": missing}))
        .await
        .expect_err("total was never returned by step 1");
    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_unresolved"), "{err:?}");
    assert_eq!(err.data["available"], json!(["count", "items"]), "{:?}", err.data);
}

#[tokio::test]
async fn a_steps_index_at_or_past_the_step_is_a_forward_reference() {
    let (_server, client) = setup_observed().await;
    let fwd = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "mid", "args": {"x": "$.steps[2].result.x"}},
        {"tool": "sink", "args": {}},
    ]});

    let dry = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "fwd", "kind": "chain", "spec": fwd, "dry_run": true}),
            )
            .await
            .expect("dry_run reports the verdict"),
    );
    let gate = dry["gates"]
        .as_array()
        .expect("gates")
        .iter()
        .find(|g| g["gate"] == json!("verdict"))
        .unwrap_or_else(|| panic!("verdict gate: {dry}"));
    assert_eq!(gate["verdict"], json!("will_fail"), "{dry}");
    assert_eq!(gate["failures"][0]["reason"], json!("forward reference"), "{dry}");

    let err = client
        .tools_call("host.tool_publish", json!({"name": "fwd", "kind": "chain", "spec": fwd}))
        .await
        .expect_err("a forward reference can never resolve");
    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_unresolved"), "{err:?}");
    assert_eq!(err.data["reason"], json!("forward reference"), "{:?}", err.data);
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
}
