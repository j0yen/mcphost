//! AC6 (P0) — Given a paused trigger, When its time passes, Then no run is
//! created; When resumed, Then the next firing runs.

use crate::common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn paused_trigger_skips_firing_resumed_trigger_fires() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "AC6 Tenant", "cus_ac6").await;
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
                json!({"tool": "pinger", "schedule": "* * * * *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let paused = extract_structured(
        &client
            .tools_call("host.trigger.pause", json!({"id": trigger_id}))
            .await
            .expect("trigger.pause"),
    );
    assert_eq!(paused["enabled"], json!(false));

    let now = mcphost::state::now_unix();
    server
        .state
        .db
        .update_trigger_after_fire(trigger_id.clone(), Some(now), None, 0)
        .await
        .expect("force due while paused");
    mcphost::triggers::tick_once(&server.state).await.expect("tick while paused");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "schedule"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert!(
        runs.iter().all(|r| r["trigger_ref"] != json!(trigger_id)),
        "a paused trigger must not fire: {runs:?}"
    );

    let resumed = extract_structured(
        &client
            .tools_call("host.trigger.resume", json!({"id": trigger_id}))
            .await
            .expect("trigger.resume"),
    );
    assert_eq!(resumed["enabled"], json!(true));

    // `next_unix` is still the same past-due timestamp from before pause
    // (resume doesn't recompute it) -- the very next tick treats it as due.
    mcphost::triggers::tick_once(&server.state).await.expect("tick after resume");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "schedule"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert!(
        runs.iter().any(|r| r["trigger_ref"] == json!(trigger_id)),
        "resumed trigger's next firing must run: {runs:?}"
    );
}
