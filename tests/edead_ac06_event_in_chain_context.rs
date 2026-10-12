//! PRD-mcphost-event-trigger-from-webhook AC6 (P0) — Given a chain with
//! step 1 `args: {"ref": "$.event.body.ref"}` bound to an event trigger,
//! When a delivery with body `{"ref": "main"}` fires it, Then step 1
//! receives `ref: "main"`; and Given the same chain called manually, Then
//! `$.event` is `null` and the mapping fails with `compose_mapping_missing`
//! naming `$.event.body.ref`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use mcphost::kinds::chain::{CONTEXT_ROOTS, step_context};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

#[tokio::test]
async fn delivery_is_event_for_a_fired_chain_and_null_for_a_manual_call() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Edead AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;
    let spec = json!({"steps": [{"tool": "sink", "args": {"ref": "$.event.body.ref"}}]});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "on_push", "kind": "chain", "spec": spec}))
            .await
            .expect("publish chain"),
    );
    let qualified = published["name"].as_str().expect("name").to_string();
    // Publish cannot settle `$.event.*`: it exists only while a delivery fires.
    assert_eq!(published["verdict"], json!("unverifiable"), "publish verdict: {published}");
    let reason = published["reason"].as_str().expect("reason");
    assert!(reason.contains("trigger kind: event"), "the hint names the trigger kind: {published}");

    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "on_push",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    let resp = reqwest::Client::new()
        .post(format!("{}/hooks/{}/on_push", server.base_url, ns))
        .body(json!({"ref": "main"}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 202);
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let run_id = accepted["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        match got["status"].as_str() {
            Some("done") => {
                assert_eq!(got["result"]["result"]["ref"], json!("main"), "step 1 got the ref: {got}");
                break;
            }
            Some("error") => panic!("event-fired chain errored: {got}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "run never finished: {got}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Manual calls: `$.event` is null, even when the caller smuggles an
    // `event` key into the args.
    for args in [json!({}), json!({"event": {"body": {"ref": "spoofed"}}})] {
        let err = client
            .tools_call(&qualified, args.clone())
            .await
            .expect_err("a manual call has no $.event");
        assert_eq!(err.error_code.as_deref(), Some("compose_mapping_missing"), "{args}: {err:?}");
        assert_eq!(err.data["path"], json!("$.event.body.ref"), "{args}: {err:?}");
    }
}

#[test]
fn context_keys_table_and_step_context_cannot_drift() {
    let built = step_context(&json!({}), None, &[]);
    let built_keys: BTreeSet<String> = built.as_object().expect("object").keys().cloned().collect();
    let table: BTreeSet<String> = CONTEXT_ROOTS.iter().map(|(k, _)| (*k).to_string()).collect();
    assert_eq!(built_keys, table, "step_context and CONTEXT_ROOTS disagree");
    assert!(built["event"].is_null(), "outside an event-fired run $.event is null");
}
