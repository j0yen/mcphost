//! PRD-mcphost-trigger-set-idempotent
//! AC5 (P0) — Given a trigger, When `host.trigger.remove(name: ...)`,
//! `pause(name)`, `fire(name)` and `test(name)` run, Then each behaves
//! identically to the id form.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn pause_fire_and_test_by_name_behave_like_by_id() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant Pause Fire Test").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "kind": "schedule", "name": "daily", "schedule": "0 1 * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    // pause(name) must disable the trigger exactly like pause(id) would.
    let paused = extract_structured(
        &client
            .tools_call("host.trigger.pause", json!({"name": "daily"}))
            .await
            .expect("pause by name"),
    );
    assert_eq!(paused["id"], json!(trigger_id), "{paused:?}");
    assert_eq!(paused["enabled"], json!(false), "{paused:?}");
    let got = extract_structured(
        &client
            .tools_call("host.trigger.get", json!({"id": trigger_id}))
            .await
            .expect("trigger.get"),
    );
    assert_eq!(got["enabled"], json!(false), "pause(name) must have actually paused it: {got:?}");

    // resume(name) re-enables it, same as resume(id) would.
    let resumed = extract_structured(
        &client
            .tools_call("host.trigger.resume", json!({"name": "daily"}))
            .await
            .expect("resume by name"),
    );
    assert_eq!(resumed["enabled"], json!(true), "{resumed:?}");

    // fire(name) enqueues a manual run, same as fire(id) would.
    let fired = extract_structured(
        &client
            .tools_call("host.trigger.fire", json!({"name": "daily"}))
            .await
            .expect("fire by name"),
    );
    assert_eq!(fired["status"], json!("queued"), "{fired:?}");
    assert_eq!(fired["manual"], json!(true), "{fired:?}");
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();
    let run = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(run["trigger_ref"], json!(trigger_id), "fire(name) must have fired THIS trigger: {run:?}");

    // remove(name) deletes it, same as remove(id) would.
    let removed = extract_structured(
        &client
            .tools_call("host.trigger.remove", json!({"name": "daily"}))
            .await
            .expect("remove by name"),
    );
    assert_eq!(removed["removed"], json!(trigger_id), "{removed:?}");
    let after = client.tools_call("host.trigger.get", json!({"id": trigger_id})).await;
    assert!(after.is_err(), "the trigger must be gone after remove(name): {after:?}");
}

#[tokio::test]
async fn test_verb_accepts_name_for_an_event_trigger() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant Test Verb").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_event", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "on_event",
                    "kind": "event",
                    "name": "inbound",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set event"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let tested = extract_structured(
        &client
            .tools_call("host.trigger.test", json!({"name": "inbound", "body": {"x": 1}}))
            .await
            .expect("test by name"),
    );
    assert_eq!(tested["status"], json!("queued"), "{tested:?}");
    assert_eq!(tested["test"], json!(true), "{tested:?}");
    let run_id = tested["run_id"].as_str().expect("run_id").to_string();
    let run = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(run["trigger_ref"], json!(trigger_id), "test(name) must have tested THIS trigger: {run:?}");

    // An unresolvable name fails trigger_not_found, same as an unresolvable
    // id would.
    let err = client
        .tools_call("host.trigger.test", json!({"name": "no-such-trigger"}))
        .await
        .expect_err("an unknown name must not resolve to any trigger");
    assert_eq!(err.error_code.as_deref(), Some("trigger_not_found"), "{err:?}");
}
