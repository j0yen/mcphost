//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC2 (P0) — Given a python tool doing `from mcphost import msg;
//! msg.send(to, {"hello": 1})` where `to` is a tenant that accepted
//! contact, When called, Then the recipient's `host.msg.inbox` shows the
//! message and the result carries the thread id.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_msg_send_reaches_inbox_and_returns_thread_id() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns_a, key_a) = signup(&server.base_url, "Chanbridge AC2 Sender").await;
    let (ns_b, key_b) = signup(&server.base_url, "Chanbridge AC2 Recipient").await;
    let client_a = common::McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = common::McpClient::with_bearer(&server.base_url, &key_b);

    // B's default contact policy is `open` (same default `msg_ac01` relies
    // on) -- "a tenant that accepted contact" with no extra consent setup.
    let spec = json!({
        "source": "from mcphost import msg\ndef main(args):\n    return msg.send(args[\"to\"], {\"hello\": 1})\n",
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "sender", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns_a}.sender");
    let result = extract_structured(
        &poll_until_ready(
            &client_a,
            &qualified,
            json!({"to": ns_b}),
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|e| panic!("tool call must succeed: {} {}", e.code, e.message)),
    );
    assert!(result["thread_id"].is_string(), "{result:?}");
    assert!(result["message_id"].is_string(), "{result:?}");

    let inbox = extract_structured(
        &client_b
            .tools_call("host.msg.inbox", json!({}))
            .await
            .expect("B inbox"),
    );
    let messages = inbox["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "{inbox:?}");
    assert_eq!(messages[0]["from_address"], json!(ns_a), "{inbox:?}");
    assert_eq!(messages[0]["thread_id"], result["thread_id"], "{inbox:?}");
}
