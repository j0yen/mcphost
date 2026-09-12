//! AC5 (P0) — Given a free trigger receiving 40 events in a minute, When
//! the 31st arrives, Then 429 events_rate_limited and no run.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn thirty_first_event_in_a_minute_is_rate_limited() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "noisy_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "noisy_hook",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let url = format!("{}/hooks/{}/noisy_hook", server.base_url, ns);

    // The free plan's own `events_per_minute` default is 30 (plans.rs).
    for i in 0..30 {
        let resp = http
            .post(&url)
            .body(json!({"n": i}).to_string())
            .send()
            .await
            .expect("POST /hooks/...");
        assert_eq!(resp.status(), 202, "event {i} within the limit must be accepted");
    }

    let resp = http
        .post(&url)
        .body(json!({"n": 30}).to_string())
        .send()
        .await
        .expect("POST /hooks/... (31st)");
    assert_eq!(resp.status(), 429);
    let error_body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(error_body["error_code"], json!("events_rate_limited"));

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event", "limit": 200}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 30, "the 31st event must not have created a run: {runs:?}");
}
