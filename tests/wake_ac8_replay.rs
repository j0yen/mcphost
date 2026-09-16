//! PRD-mcphost-agent-wake
//! AC8 (P0) — Given a message-fired run failed, When R calls
//! host.trigger.replay(run_id), Then a new run executes with the identical
//! envelope and the original run is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn replay_of_a_failed_message_triggered_run_reuses_the_stored_envelope() {
    let server = TestServer::start_with_kinds(common::chain_kind_registry()).await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC8 Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC8 Sender").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);

    // A tool that always errors (chain step naming a tool that doesn't
    // exist -- same trick `hooks_ac8_trigger_replay_new_run_trigger_ref.rs`
    // uses for the event-kind version of this AC), so the message-fired run
    // lands `error` and is worth replaying.
    client_r
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "broken_handler",
                "kind": "chain",
                "spec": {"steps": [{"tool": "does_not_exist", "args": {}}]},
            }),
        )
        .await
        .expect("publish");
    client_r
        .tools_call("host.trigger.set", json!({"tool": "broken_handler", "kind": "message"}))
        .await
        .expect("trigger.set");

    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "will fail"}))
        .await
        .expect("S sends");

    let original_run_id = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let list = extract_structured(
                &client_r
                    .tools_call("host.runs.list", json!({"trigger": "message"}))
                    .await
                    .expect("runs.list"),
            );
            let runs = list["runs"].as_array().expect("runs array");
            if let Some(run) = runs.first()
                && run["status"] == json!("error")
            {
                break run["run_id"].as_str().expect("run_id").to_string();
            }
            assert!(Instant::now() < deadline, "message-triggered run never reached error: {list:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };

    let replayed = extract_structured(
        &client_r
            .tools_call("host.trigger.replay", json!({"run_id": original_run_id}))
            .await
            .expect("trigger.replay"),
    );
    let new_run_id = replayed["run_id"].as_str().expect("run_id").to_string();
    assert_ne!(new_run_id, original_run_id);

    let new_run = extract_structured(
        &client_r
            .tools_call("host.runs.get", json!({"run_id": new_run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(new_run["trigger"], json!("message"), "{new_run:?}");
    assert_eq!(
        new_run["trigger_ref"],
        json!(original_run_id),
        "the replay's trigger_ref must name the original run: {new_run:?}"
    );

    // The original run is unchanged: still error, still the same run id.
    let original_after = extract_structured(
        &client_r
            .tools_call("host.runs.get", json!({"run_id": original_run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(original_after["status"], json!("error"), "{original_after:?}");
    assert_eq!(original_after["run_id"], json!(original_run_id), "{original_after:?}");
}
