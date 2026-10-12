//! PRD-mcphost-event-trigger-from-webhook AC4 (P0) — Given the letter of AC1
//! after the rate window, When `host.trigger.replay(dead_letter_id)` is
//! called, Then a new run is enqueued with the stored body, the letter's
//! `replayed_run_id` is set, and a second replay enqueues a second run (no
//! dedupe).

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn replaying_a_rate_limited_letter_runs_its_body_every_time() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Edead AC4 Tenant").await;
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
        let resp = http.post(&url).body(json!({"n": i}).to_string()).send().await.expect("POST");
        assert_eq!(resp.status(), 202);
    }
    let thirty_first = json!({"n": 30, "marker": "the-31st"});
    let resp = http.post(&url).body(thirty_first.to_string()).send().await.expect("POST 31st");
    assert_eq!(resp.status(), 429);
    let refusal: serde_json::Value = resp.json().await.expect("json body");
    let letter_id = refusal["error"]["dead_letter_id"].as_str().expect("dead_letter_id").to_string();

    // The replay does not pass through the rate limiter, so it works inside
    // the same window the 31st delivery was refused in.
    let first = extract_structured(
        &client
            .tools_call("host.trigger.replay", json!({"dead_letter_id": letter_id}))
            .await
            .expect("replay"),
    );
    let first_run = first["run_id"].as_str().expect("run_id").to_string();

    // The executor leases one run per tenant per tick, so the replay waits
    // behind the 30 deliveries accepted before it.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": first_run}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(got["result"]["event"]["body"], thirty_first, "{got:?}");
            assert_eq!(got["trigger"], json!("event"));
            break;
        }
        assert!(Instant::now() < deadline, "replayed run never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.dead_letters", json!({"trigger": "gh_push"}))
            .await
            .expect("dead_letters"),
    );
    assert_eq!(listed["dead_letters"][0]["replayed_run_id"], json!(first_run));

    let second = extract_structured(
        &client
            .tools_call("host.trigger.replay", json!({"dead_letter_id": letter_id}))
            .await
            .expect("second replay"),
    );
    let second_run = second["run_id"].as_str().expect("run_id");
    assert_ne!(second_run, first_run, "a second replay is a second run");
}
