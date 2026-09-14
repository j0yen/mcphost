//! PRD-mcphost-agent-inbox
//! AC7 (P0) — Given a send whose arguments include `from: "@someone"`,
//! When it is submitted, Then the response is `args_invalid` and nothing
//! is stored.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac7_forged_from_argument_is_rejected_and_stores_nothing() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let err = client_a
        .tools_call(
            "host.msg.send",
            json!({"to": [ns_b.clone()], "body": "ping", "from": "@someone"}),
        )
        .await
        .expect_err("forged from must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 0, "{inbox:?}");
}
