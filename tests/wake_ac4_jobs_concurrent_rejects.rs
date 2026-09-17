//! PRD-mcphost-agent-wake
//! AC4 (P0) — Given R's plan has jobs_concurrent=1 and one run is in
//! flight, When S sends R a second message, Then the message is stored and
//! readable in R's inbox, and a run row exists with status="rejected" and
//! a non-empty error_class.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn jobs_concurrent_at_cap_rejects_the_message_triggered_run_but_still_stores_the_message() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC4 Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC4 Sender").await;
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

    // The free plan's own jobs_concurrent is 1 (see plans.rs's default
    // catalog) -- occupy that one slot directly via the db (rather than a
    // slow python-sandbox job, which needs user namespaces this box may
    // not have) so the message trigger's own enqueue sees the tenant
    // already at capacity.
    let tenant_r = server
        .state
        .db
        .find_tenant_by_namespace(ns_r.clone())
        .await
        .expect("db lookup")
        .expect("tenant exists");
    server
        .state
        .db
        .insert_queued_run(
            "wake-ac4-occupier".to_string(),
            tenant_r.id,
            "occupier".to_string(),
            "job".to_string(),
            None,
            None,
            300,
            "{}".to_string(),
            false,
            false,
            None,
        )
        .await
        .expect("insert occupier run");
    server
        .state
        .db
        .lease_next_queued_run(tenant_r.id)
        .await
        .expect("lease occupier run")
        .expect("occupier run leased into running");

    let sent = extract_structured(
        &client_s
            .tools_call("host.msg.send", json!({"to": [ns_r], "body": "at capacity"}))
            .await
            .expect("send"),
    );
    assert_eq!(sent["delivered_to"], json!([ns_r]), "{sent:?}");
    let message_id = sent["message_id"].as_str().expect("message_id").to_string();

    // The message must be stored and readable in R's inbox regardless of
    // the trigger's own enqueue outcome.
    let inbox = extract_structured(
        &client_r.tools_call("host.msg.inbox", json!({})).await.expect("inbox"),
    );
    let messages = inbox["messages"].as_array().expect("messages array");
    assert!(
        messages.iter().any(|m| m["message_id"] == json!(message_id)),
        "the message must still be stored even though the trigger's run was rejected: {inbox:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let list = extract_structured(
            &client_r
                .tools_call("host.runs.list", json!({"trigger": "message"}))
                .await
                .expect("runs.list"),
        );
        let runs = list["runs"].as_array().expect("runs array");
        if let Some(run) = runs.first() {
            assert_eq!(run["status"], json!("rejected"), "{run:?}");
            assert!(
                run["error_class"].as_str().is_some_and(|s| !s.is_empty()),
                "a rejected run must carry a non-empty error_class: {run:?}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "no message-triggered run ever appeared: {list:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
