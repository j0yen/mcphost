//! PRD-mcphost-event-trigger-from-webhook AC3 (P0) — Given an event-fired
//! run whose tool raises, When the run finalizes, Then a letter with
//! `reason_code: run_error`, `run_id`, and `error_class` exists and
//! `host.trigger.get` reports `dead_letters.count` 1.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn failed_event_run_leaves_a_run_error_letter() {
    let server = common::TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Edead AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // A chain whose only step names a tool that is removed before the
    // delivery arrives: dispatch raises, the run finalizes `error`.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gone_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish gone_tool");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "broken_hook", "kind": "chain", "spec": {"steps": [{"tool": "gone_tool", "args": {}}]}}),
        )
        .await
        .expect("publish broken_hook");
    client
        .tools_call("host.tool_remove", json!({"name": "gone_tool"}))
        .await
        .expect("remove gone_tool");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "broken_hook",
                    "name": "broken",
                    "kind": "event",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("trigger id").to_string();

    let resp = reqwest::Client::new()
        .post(format!("{}/hooks/{}/broken_hook", server.base_url, ns))
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 202);
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let run_id = accepted["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    let letter = loop {
        let listed = extract_structured(
            &client
                .tools_call("host.trigger.dead_letters", json!({"trigger": "broken"}))
                .await
                .expect("dead_letters"),
        );
        if let Some(first) = listed["dead_letters"].as_array().and_then(|a| a.first()) {
            break first.clone();
        }
        assert!(Instant::now() < deadline, "no run_error letter appeared");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(letter["reason_code"], json!("run_error"));
    assert_eq!(letter["run_id"], json!(run_id));
    assert!(
        letter["error_class"].as_str().is_some_and(|c| !c.is_empty()),
        "error_class must be set: {letter:?}"
    );

    let got = extract_structured(
        &client
            .tools_call("host.trigger.get", json!({"id": trigger_id}))
            .await
            .expect("trigger.get"),
    );
    assert_eq!(got["dead_letters"]["count"], json!(1), "{got:?}");
}
