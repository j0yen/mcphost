//! AC2 (P0) — Given `schedule="61 * * * *"`, When set, Then the error is
//! `trigger_invalid` naming the minute field.

mod common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn out_of_range_minute_is_trigger_invalid_naming_minute() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "pinger", "schedule": "61 * * * *"}),
        )
        .await
        .expect_err("out-of-range minute must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("trigger_invalid"));
    assert_eq!(err.data["field"], json!("minute"), "{:?}", err.data);
}
