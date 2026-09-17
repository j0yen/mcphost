//! PRD-mcphost-agent-wake
//! AC5 (P0) — Given a paused message trigger, When S sends R a message,
//! Then no run is enqueued; and after host.trigger.resume, When S sends
//! again, Then one run is.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_paused_message_trigger_does_not_fire_until_resumed() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC5 Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC5 Sender").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);

    client_r
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client_r
            .tools_call("host.trigger.set", json!({"tool": "handle_msg", "kind": "message"}))
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let paused = extract_structured(
        &client_r
            .tools_call("host.trigger.pause", json!({"id": trigger_id}))
            .await
            .expect("trigger.pause"),
    );
    assert_eq!(paused["enabled"], json!(false), "{paused:?}");

    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "while paused"}))
        .await
        .expect("S sends while paused");
    let list = extract_structured(
        &client_r
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    assert_eq!(
        list["runs"].as_array().expect("runs array").len(),
        0,
        "a paused trigger must not fire: {list:?}"
    );

    let resumed = extract_structured(
        &client_r
            .tools_call("host.trigger.resume", json!({"id": trigger_id}))
            .await
            .expect("trigger.resume"),
    );
    assert_eq!(resumed["enabled"], json!(true), "{resumed:?}");

    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "after resume"}))
        .await
        .expect("S sends after resume");
    let list = extract_structured(
        &client_r
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    let runs = list["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "a resumed trigger must fire on the next send: {list:?}");
}
