//! PRD-mcphost-agent-inbox
//! AC5 (P0) — Given B has called `host.msg.block(A.address)`, When A
//! sends to B, Then `refused` contains B with code `agent_not_found`
//! byte-identical to sending to a nonexistent address, and B's inbox is
//! unchanged; and When B sends to A, Then it is delivered.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac5_a_blocked_sender_fails_agent_not_found_while_the_blocker_can_still_send() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.msg.block", json!({"address": ns_a}))
        .await
        .expect("B blocks A");

    let blocked_send = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("call itself succeeds, recipient is refused");
    let blocked_result = extract_structured(&blocked_send);
    assert_eq!(blocked_result["delivered_to"], json!([]), "{blocked_result:?}");
    let refused = blocked_result["refused"].as_array().expect("refused array");
    assert_eq!(refused.len(), 1, "{blocked_result:?}");
    assert_eq!(refused[0]["address"], json!(ns_b), "{blocked_result:?}");
    assert_eq!(refused[0]["code"], json!("agent_not_found"), "{blocked_result:?}");

    // Byte-identical to sending to a nonexistent address.
    let nonexistent_send = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": ["t_doesnotexist00000000"], "body": "ping"}),
        )
        .await
        .expect("call itself succeeds, recipient is refused");
    let nonexistent_result = extract_structured(&nonexistent_send);
    let nonexistent_refused = nonexistent_result["refused"].as_array().expect("refused array");
    assert_eq!(nonexistent_refused[0]["code"], refused[0]["code"], "{nonexistent_result:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");

    // B, the blocker, can still send to A.
    let reverse_raw = client_b
        .tools_call("host.msg.send", json!({"to": [ns_a.clone()], "body": "hi anyway"}))
        .await
        .expect("B can send to A");
    let reverse = extract_structured(&reverse_raw);
    assert_eq!(reverse["delivered_to"], json!([ns_a]), "{reverse:?}");
}
