//! PRD-mcphost-event-trigger-from-webhook AC1 (P0) — Given an event trigger
//! on a free-plan tenant, When 31 valid deliveries arrive within 60 s, Then
//! the 31st answers 429 with `dead_letter_id` in the body and
//! `host.trigger.dead_letters(trigger)` lists one row with `reason_code:
//! events_rate_limited` and `body` equal to the 31st delivery.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn thirty_first_delivery_is_a_dead_letter_with_its_body() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Edead AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "burst_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "burst_hook",
                "name": "gh_push",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let url = format!("{}/hooks/{}/burst_hook", server.base_url, ns);
    for i in 0..30 {
        let resp = http
            .post(&url)
            .body(json!({"n": i}).to_string())
            .send()
            .await
            .expect("POST");
        assert_eq!(resp.status(), 202, "delivery {i} is within the limit");
    }

    let thirty_first = json!({"n": 30, "marker": "the-31st"}).to_string();
    let resp = http.post(&url).body(thirty_first.clone()).send().await.expect("POST 31st");
    assert_eq!(resp.status(), 429);
    let refusal: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(refusal["error_code"], json!("events_rate_limited"));
    assert_eq!(refusal["error"]["code"], json!("events_rate_limited"));
    let letter_id = refusal["error"]["dead_letter_id"].as_str().expect("dead_letter_id in body");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.dead_letters", json!({"trigger": "gh_push"}))
            .await
            .expect("dead_letters"),
    );
    let letters = listed["dead_letters"].as_array().expect("dead_letters array");
    assert_eq!(letters.len(), 1, "exactly one letter: {letters:?}");
    assert_eq!(letters[0]["dead_letter_id"], json!(letter_id));
    assert_eq!(letters[0]["reason_code"], json!("events_rate_limited"));
    assert_eq!(letters[0]["body"], json!(thirty_first));
}
