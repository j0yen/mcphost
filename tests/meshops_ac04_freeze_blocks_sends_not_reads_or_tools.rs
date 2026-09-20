//! PRD-mcphost-agent-mesh-ops
//! AC4 (P0) -- Given tenant A is frozen with `admin.mesh.freeze(A,
//! reason="flood")`, When A calls `host.msg.send`, `host.msg.reply`,
//! `host.channel.post` and `host.agent.contact_request`, Then each
//! returns `mesh_frozen`; and When A calls `host.msg.inbox`,
//! `host.msg.wait`, `host.tool_call` on its own tool, and another tenant
//! calls A's shared tool and sends A a message, Then all succeed and the
//! message is delivered.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac4_freeze_blocks_outbound_mesh_calls_but_not_tools_reads_or_inbound() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Setup that must exist BEFORE the freeze: an existing thread with A
    // as a participant (for the `reply` case), A's own published+shared
    // tool (for the "own tool call" and "another tenant calls A's shared
    // tool" cases), and a channel A has already opened (`host.channel.open`
    // is not itself gated by requirement 4 -- only `host.channel.post` is).
    let pre_freeze_raw = client_b
        .tools_call("host.msg.send", json!({"to": [ns_a.clone()], "body": "before freeze"}))
        .await
        .expect("B sends A a message before the freeze");
    let thread_id = extract_structured(&pre_freeze_raw)["thread_id"]
        .as_str()
        .expect("thread_id")
        .to_string();

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes echo_tool");
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "echo_tool", "visibility": "public", "description": "echo"}),
        )
        .await
        .expect("A shares echo_tool publicly");

    client_a
        .tools_call("host.channel.open", json!({"name": "general"}))
        .await
        .expect("A opens a channel before the freeze");

    // Freeze A.
    let freeze_raw = admin
        .tools_call("admin.mesh.freeze", json!({"tenant": ns_a.clone(), "reason": "flood"}))
        .await
        .expect("admin.mesh.freeze");
    assert_eq!(extract_structured(&freeze_raw)["mesh_frozen"], json!(true));

    // Every one of these must return `mesh_frozen`.
    let send_err = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "should not send"}))
        .await
        .expect_err("frozen send must fail");
    assert_eq!(send_err.error_code.as_deref(), Some("mesh_frozen"), "{send_err:?}");

    let reply_err = client_a
        .tools_call(
            "host.msg.reply",
            json!({"thread_id": thread_id, "body": "should not reply"}),
        )
        .await
        .expect_err("frozen reply must fail");
    assert_eq!(reply_err.error_code.as_deref(), Some("mesh_frozen"), "{reply_err:?}");

    let post_err = client_a
        .tools_call("host.channel.post", json!({"channel": "general", "body": "should not post"}))
        .await
        .expect_err("frozen channel post must fail");
    assert_eq!(post_err.error_code.as_deref(), Some("mesh_frozen"), "{post_err:?}");

    let contact_err = client_a
        .tools_call("host.agent.contact_request", json!({"address": ns_b.clone()}))
        .await
        .expect_err("frozen contact_request must fail");
    assert_eq!(contact_err.error_code.as_deref(), Some("mesh_frozen"), "{contact_err:?}");

    // Every one of these must still succeed.
    let inbox_raw = client_a
        .tools_call("host.msg.inbox", json!({}))
        .await
        .expect("frozen tenant can still read its inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().expect("messages array").len(), 1, "{inbox:?}");

    client_a
        .tools_call("host.msg.wait", json!({"timeout_s": 1}))
        .await
        .expect("frozen tenant can still call host.msg.wait");

    let own_call_raw = client_a
        .tools_call("host.tool_call", json!({"name": "echo_tool", "args": {"msg": "hi"}}))
        .await
        .expect("frozen tenant can still call its own tool");
    assert_eq!(extract_structured(&own_call_raw), json!({"msg": "hi"}));

    // Another tenant can still call A's shared tool while A is frozen.
    let qualified = format!("{ns_a}.echo_tool");
    let cross_call_raw = client_b
        .tools_call(&qualified, json!({"msg": "hello"}))
        .await
        .expect("B can still call A's shared tool while A is frozen");
    assert_eq!(extract_structured(&cross_call_raw), json!({"msg": "hello"}));

    // Another tenant can still send A a message while A is frozen, and it
    // is delivered.
    let deliver_raw = client_b
        .tools_call("host.msg.send", json!({"to": [ns_a.clone()], "body": "still delivered"}))
        .await
        .expect("B can still send to frozen A");
    assert_eq!(extract_structured(&deliver_raw)["delivered_to"], json!([ns_a]));
}
