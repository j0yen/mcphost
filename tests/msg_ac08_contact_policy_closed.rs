//! PRD-mcphost-agent-inbox
//! AC8 (P0) — Given B's policy is `closed`, When A sends to B, Then
//! `refused` contains B with `contact_refused` and B's inbox is empty.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac8_closed_contact_policy_refuses_the_send() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_b
        .tools_call("host.agent.profile_set", json!({"contact_policy": "closed"}))
        .await
        .expect("B closes contact policy");

    let raw = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "ping"}))
        .await
        .expect("call succeeds, recipient is refused");
    let result = extract_structured(&raw);
    assert_eq!(result["delivered_to"], json!([]), "{result:?}");
    let refused = result["refused"].as_array().expect("refused array");
    assert_eq!(refused[0]["address"], json!(ns_b), "{result:?}");
    assert_eq!(refused[0]["code"], json!("contact_refused"), "{result:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
}
