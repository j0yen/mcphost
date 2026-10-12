//! PRD-mcphost-chain-fanout-and-join AC5 (P0) -- Given a predecessor whose
//! observed shape has `items: object`, When a chain with `for_each:
//! "$.prev.result.items"` is published, Then `will_fail` with `expected:
//! "array"`; Given a `$.item` reference in a step without `for_each`, Then
//! publish is refused with `compose_item_outside_for_each`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// One async call of `tool`, polled to a terminal `done` status, so the
/// publish gate learns the tool's observed result shape.
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
async fn a_for_each_over_an_observed_object_is_will_fail_expecting_an_array() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    run_to_done(&client, "src", json!({"items": {"a": 1}})).await;

    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
    ]});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "fan", "kind": "chain", "spec": spec}))
        .await
        .expect_err("a for_each over an object must be refused at publish");
    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_unresolved"), "{err:?}");
    assert_eq!(err.data["expected"], json!("array"), "{:?}", err.data);
    assert_eq!(err.data["actual"], json!("object"), "{:?}", err.data);
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["path"], json!("$.prev.result.items"), "{:?}", err.data);

    // The dry-run publish reports the same verdict without refusing.
    let dry = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "fan", "kind": "chain", "spec": spec, "dry_run": true}))
            .await
            .expect("dry_run reports"),
    );
    assert!(dry.to_string().contains("will_fail"), "{dry}");
    assert!(dry.to_string().contains("\"expected\":\"array\""), "{dry}");

    // An array-valued field publishes.
    run_to_done(&client, "src", json!({"rows": [1, 2]})).await;
    let ok = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"n": "$.item"}, "for_each": "$.prev.result.rows"},
    ]});
    client
        .tools_call("host.tool_publish", json!({"name": "fan_ok", "kind": "chain", "spec": ok}))
        .await
        .expect("an array-valued for_each publishes");
}

#[tokio::test]
async fn item_outside_for_each_is_refused_at_publish() {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC5b Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;

    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"n": "$.item"}},
    ]});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "stray", "kind": "chain", "spec": spec}))
        .await
        .expect_err("$.item outside for_each must be refused");
    assert_eq!(err.error_code.as_deref(), Some("compose_item_outside_for_each"), "{err:?}");
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
}
