//! AC9 (P0) — Given a paused trigger, When an event arrives, Then 404
//! hook_not_found and no run.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn paused_event_trigger_answers_404_hook_not_found() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pausable_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "pausable_hook",
                    "kind": "event",
                    "verify": {"scheme": "none", "allow_unverified": true},
                }),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    client
        .tools_call("host.trigger.pause", json!({"id": trigger_id}))
        .await
        .expect("trigger.pause");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hooks/{}/pausable_hook", server.base_url, ns))
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 404);
    let error_body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(error_body["error_code"], json!("hook_not_found"));

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert!(runs.is_empty(), "a paused trigger must create no run: {runs:?}");

    // Also 404 for a namespace/tool that never existed at all -- same code,
    // no information leak about which part of the path was wrong.
    let resp2 = http
        .post(format!("{}/hooks/{}/no_such_tool", server.base_url, ns))
        .body("{}")
        .send()
        .await
        .expect("POST /hooks/... unknown tool");
    assert_eq!(resp2.status(), 404);
}
