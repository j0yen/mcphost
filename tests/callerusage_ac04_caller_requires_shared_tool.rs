//! PRD-mcphost-shared-tool-caller-usage
//! AC4 (P0) — Given `by: "caller"` on a tool that is not shared, When
//! `host.usage` runs, Then it is rejected with a validation error naming
//! the constraint.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn caller_breakdown_on_a_private_tool_is_rejected() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Owner O").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "private_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish private_tool");

    let err = client
        .tools_call(
            "host.usage",
            json!({"tool": "private_tool", "by": "caller", "window": "1d"}),
        )
        .await
        .expect_err("by: 'caller' on a private tool must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    assert!(
        err.message.contains("shared") || err.message.contains("private"),
        "the error must name the shared-tool constraint: {}",
        err.message
    );
}
