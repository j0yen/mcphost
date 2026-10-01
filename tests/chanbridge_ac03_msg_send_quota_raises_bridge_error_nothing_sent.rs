//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC3 (P0) — Given a tenant at its plan's message quota, When the tool
//! calls `msg.send`, Then Python receives `McphostBridgeError` with
//! `.code` equal to the external verb's error class and nothing is sent.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_msg_send_at_quota_raises_bridge_error_and_sends_nothing() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns_a, key_a) = signup(&server.base_url, "Chanbridge AC3 Sender").await;
    let (ns_b, key_b) = signup(&server.base_url, "Chanbridge AC3 Recipient").await;
    let client_a = common::McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = common::McpClient::with_bearer(&server.base_url, &key_b);

    // Fill A's free-plan `msgs_per_hour` (60, same constant
    // `msg_ac09_msgs_per_hour_quota.rs` pins) with plain, un-sandboxed
    // sends -- quicker than publishing a tool for each, and this AC is
    // about the 61st send specifically, not how the quota got full.
    for n in 0..60 {
        client_a
            .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("msg {n}")}))
            .await
            .unwrap_or_else(|e| panic!("send {n} should succeed: {e:?}"));
    }

    let spec = json!({
        "source": "import mcphost\nfrom mcphost import msg\ndef main(args):\n    try:\n        msg.send(args[\"to\"], {\"hello\": 1})\n        return {\"raised\": False}\n    except mcphost.msg.BridgeError as e:\n        return {\"raised\": True, \"is_bridge_error\": isinstance(e, mcphost.BridgeError), \"code\": e.code, \"data\": e.data}\n",
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
    assert_eq!(result["raised"], json!(true), "{result:?}");
    assert_eq!(result["is_bridge_error"], json!(true), "{result:?}");
    assert_eq!(result["code"], json!("quota_exceeded"), "{result:?}");
    assert_eq!(result["data"]["limit"], json!("msgs_per_hour"), "{result:?}");

    let inbox = extract_structured(
        &client_b
            .tools_call("host.msg.inbox", json!({"limit": 100}))
            .await
            .expect("B inbox"),
    );
    assert_eq!(
        inbox["messages"].as_array().expect("messages array").len(),
        60,
        "{inbox:?}"
    );
}
