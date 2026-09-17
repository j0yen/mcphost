//! PRD-mcphost-agent-wake
//! AC2 (P0) — Given the AC1 trigger with from set to tenant T's address,
//! When S sends R a message, Then no run is enqueued; and When T sends,
//! Then one run is.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn from_filter_only_fires_for_the_named_sender() {
    let server = TestServer::start().await;
    let (ns_r, key_r) = signup(&server.base_url, "Wake AC2 Recipient").await;
    let (_ns_s, key_s) = signup(&server.base_url, "Wake AC2 Sender").await;
    let (ns_t, key_t) = signup(&server.base_url, "Wake AC2 Trusted").await;
    let client_r = McpClient::with_bearer(&server.base_url, &key_r);
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);
    let client_t = McpClient::with_bearer(&server.base_url, &key_t);

    client_r
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client_r
            .tools_call(
                "host.trigger.set",
                json!({"tool": "handle_msg", "kind": "message", "from": ns_t}),
            )
            .await
            .expect("trigger.set"),
    );
    assert_eq!(set["from"], json!(ns_t), "{set:?}");

    // S (not the trusted sender) sends -- no run must appear. `send`
    // awaits `fire_message_triggers` before returning, so no polling wait
    // is needed to prove absence: by the time this call returns, the
    // enqueue-or-skip decision already happened.
    client_s
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "untrusted"}))
        .await
        .expect("S sends");
    let list = extract_structured(
        &client_r
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    assert_eq!(
        list["runs"].as_array().expect("runs array").len(),
        0,
        "an untrusted sender must not fire the trigger: {list:?}"
    );

    // T (the trusted sender) sends -- exactly one run must appear.
    client_t
        .tools_call("host.msg.send", json!({"to": [ns_r], "body": "trusted"}))
        .await
        .expect("T sends");
    let list = extract_structured(
        &client_r
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    let runs = list["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "the trusted sender must fire exactly one run: {list:?}");
}
