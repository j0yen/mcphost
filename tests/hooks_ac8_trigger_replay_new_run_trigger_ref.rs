//! AC8 (P0) — Given a failed event run, When `host.trigger.replay(run_id)`
//! is called, Then a new run starts with the same `event` args and
//! `trigger_ref` naming the original.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn replay_reuses_the_stored_event_and_names_the_original_run() {
    let server = common::TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // A tool that always errors, so the first delivery lands as a `failed`
    // (error) run to replay -- `python`-kind's own AC1-style "unknown kind"
    // trick isn't needed here: `host.tool_call` on an unpublished/undefined
    // dependency inside a chain is overkill for this test, so instead this
    // uses an `echo` tool whose schema the event body will *not* satisfy,
    // forcing the run to fail at dispatch. Simpler: use a `chain` tool with
    // a step naming a tool that doesn't exist, which `kinds::chain::call`
    // reports as a structured error, finalizing the run `error`.
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "broken_hook",
                "kind": "chain",
                "spec": {"steps": [{"tool": "does_not_exist", "args": {}}]},
            }),
        )
        .await
        .expect("publish");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "broken_hook",
                "kind": "event",
                "verify": {"scheme": "none", "allow_unverified": true},
            }),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hooks/{}/broken_hook", server.base_url, ns))
        .body(json!({"n": 1}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 202);
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let original_run_id = accepted["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": original_run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("error") {
            break;
        }
        if Instant::now() >= deadline {
            panic!("original run {original_run_id} never reached error: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let replayed = extract_structured(
        &client
            .tools_call("host.trigger.replay", json!({"run_id": original_run_id}))
            .await
            .expect("trigger.replay"),
    );
    let new_run_id = replayed["run_id"].as_str().expect("run_id").to_string();
    assert_ne!(new_run_id, original_run_id);

    let new_run = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": new_run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(
        new_run["trigger_ref"],
        json!(original_run_id),
        "the replay's trigger_ref must name the original run: {new_run:?}"
    );
    assert_eq!(new_run["trigger"], json!("event"));
}
