//! PRD-mcphost-sandbox-channel-msg-bridge
//! AC4 (P0) — Given the same tool run under `host.tool_test`, When it
//! calls `channel.post`, Then the return is `{delivered: false,
//! would_post: {...}}`, `host.channel.read` shows no new post, and no
//! agent-wake trigger fires.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn mcphost_channel_post_under_tool_test_is_a_dry_run() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns_a, key_a) = signup(&server.base_url, "Chanbridge AC4 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "Chanbridge AC4 Member").await;
    let client_a = common::McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = common::McpClient::with_bearer(&server.base_url, &key_b);

    client_a
        .tools_call("host.group.create", json!({"name": "g"}))
        .await
        .expect("group.create");
    client_a
        .tools_call("host.group.add", json!({"name": "g", "namespace": ns_b.clone()}))
        .await
        .expect("group.add");
    let opened = extract_structured(
        &client_a
            .tools_call("host.channel.open", json!({"group": "g"}))
            .await
            .expect("channel.open"),
    );
    let channel_id = opened["channel_id"].as_str().expect("channel_id").to_string();

    // B binds a message trigger to this channel (PRD-mcphost-agent-wake) --
    // AC4's "no agent-wake trigger fires" needs a real trigger that COULD
    // fire to be a meaningful assertion.
    client_b
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_msg", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish handle_msg");
    client_b
        .tools_call(
            "host.trigger.set",
            json!({"tool": "handle_msg", "kind": "message", "channel_id": channel_id}),
        )
        .await
        .expect("trigger.set");

    let spec = json!({
        "source": "from mcphost import channel\ndef main(args):\n    return channel.post(args[\"cid\"], {\"n\": 1})\n",
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "poster", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let tested = extract_structured(
        &client_a
            .tools_call(
                "host.tool_test",
                json!({"name": "poster", "args": {"cid": channel_id}}),
            )
            .await
            .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message)),
    );
    let tool_result = &tested["result"];
    assert_eq!(tool_result["delivered"], json!(false), "{tested:?}");
    assert!(tool_result["would_post"].is_object(), "{tested:?}");
    assert_eq!(tool_result["would_post"]["channel"], json!(channel_id), "{tested:?}");

    let read = extract_structured(
        &client_a
            .tools_call("host.channel.read", json!({"channel_id": channel_id}))
            .await
            .expect("host.channel.read"),
    );
    assert_eq!(read["posts"].as_array().expect("posts array").len(), 0, "{read:?}");

    let runs = extract_structured(
        &client_b
            .tools_call("host.runs.list", json!({"trigger": "message"}))
            .await
            .expect("runs.list"),
    );
    assert_eq!(runs["runs"].as_array().expect("runs array").len(), 0, "{runs:?}");
}
