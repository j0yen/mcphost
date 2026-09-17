//! PRD-mcphost-agent-wake
//! AC3 (P0) — Given the AC1 trigger, When the same message is delivered
//! twice through the internal delivery path (simulated retry), Then
//! exactly one run exists for its message_id and the second attempt
//! returns the first run_id.
//!
//! `host.msg.send`'s own `dedupe_key` (PRD-mcphost-agent-inbox requirement
//! 8) is the simulated retry here: a second `send` with the same
//! `dedupe_key` returns the identical `message_id` without storing a new
//! message, which drives `fire_message_triggers` a second time with that
//! same `message_id` -- exactly "the same message ... delivered twice
//! through the internal delivery path". The trigger-level
//! `(trigger_id, message_id)` dedupe (`event_dedupe`, reused from the
//! event-trigger PRD) is what this test actually proves.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_redelivered_message_id_reuses_the_first_run_and_enqueues_nothing_new() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC3 Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC3 Sender").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);

    client_r
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client_r
        .tools_call("host.trigger.set", json!({"tool": "handle_msg", "kind": "message"}))
        .await
        .expect("trigger.set");

    let first = extract_structured(
        &client_s
            .tools_call(
                "host.msg.send",
                json!({"to": [ns_r], "body": "retry me", "dedupe_key": "wake-ac3"}),
            )
            .await
            .expect("first send"),
    );
    let second = extract_structured(
        &client_s
            .tools_call(
                "host.msg.send",
                json!({"to": [ns_r], "body": "retry me", "dedupe_key": "wake-ac3"}),
            )
            .await
            .expect("second (retried) send"),
    );
    assert_eq!(
        first["message_id"], second["message_id"],
        "a resend with the same dedupe_key must return the original message_id: {first:?} / {second:?}"
    );

    // Give the executor a brief moment to finish the (near-instant echo)
    // run so a slow finalize can't be mistaken for a real second run.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let list = extract_structured(
            &client_r
                .tools_call("host.runs.list", json!({"trigger": "message"}))
                .await
                .expect("runs.list"),
        );
        let runs = list["runs"].as_array().expect("runs array");
        assert!(
            runs.len() <= 1,
            "a redelivered message_id must never produce a second run: {list:?}"
        );
        if runs.len() == 1 && runs[0]["status"] == json!("done") {
            return;
        }
        assert!(Instant::now() < deadline, "message-triggered run never finished: {list:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
