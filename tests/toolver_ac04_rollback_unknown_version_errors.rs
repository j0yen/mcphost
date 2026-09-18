//! AC4 — Given `host.tool_rollback {version: 9}` for a tool with 2
//! versions, When called, Then an argument error names the valid range.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn rollback_to_unknown_version_names_the_valid_range() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "greet", "echo", json!({"schema": {"type": "object"}})).await;

    let err = client
        .tools_call("host.tool_rollback", json!({"name": "greet", "version": 9}))
        .await
        .expect_err("version 9 does not exist for a 2-version tool");

    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    assert!(
        err.message.contains('1') && err.message.contains('2'),
        "error must name the valid range (1-2): {}",
        err.message
    );
}
