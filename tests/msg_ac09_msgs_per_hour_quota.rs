//! PRD-mcphost-agent-inbox
//! AC9 (P0) — Given the free plan's `msgs_per_hour` is 60, When A sends
//! 61 messages within an hour, Then the 61st returns `quota_exceeded`
//! with `data.limit="msgs_per_hour"` and `data.value=60`, and B's inbox
//! holds 60.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac9_msgs_per_hour_quota_blocks_the_61st_send() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Agent A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Agent B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    for n in 0..60 {
        client_a
            .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": format!("msg {n}")}))
            .await
            .unwrap_or_else(|e| panic!("send {n} should succeed: {e:?}"));
    }

    let err = client_a
        .tools_call("host.msg.send", json!({"to": [ns_b.clone()], "body": "one too many"}))
        .await
        .expect_err("the 61st send must be refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!("msgs_per_hour"), "{err:?}");
    assert_eq!(err.data["value"], json!(60), "{err:?}");

    let inbox_raw = client_b.tools_call("host.msg.inbox", json!({"limit": 100})).await.expect("B inbox");
    let inbox = extract_structured(&inbox_raw);
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 60, "{inbox:?}");
}
