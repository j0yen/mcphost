//! PRD-mcphost-tool-versions
//! AC4 (P0) — Given `host.tool_rollback {version: 9}` for a tool with 2
//! versions, When called, Then an argument error names the valid range.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn rollback_to_unknown_version_is_an_argument_error_naming_the_range() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 1..=2 {
        client
            .tools_call(
                "host.tool_publish",
                json!({
                    "name": "twoversions",
                    "kind": "echo",
                    "spec": {"schema": {"type": "object", "properties": {"f": {"type": "string", "enum": [format!("v{i}")]}}, "required": ["f"]}},
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("publish v{i} should succeed: {} {}", e.code, e.message));
    }

    let err = client
        .tools_call("host.tool_rollback", json!({"name": "twoversions", "version": 9}))
        .await
        .expect_err("rollback to an out-of-range version must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
    assert!(
        err.message.contains('1') && err.message.contains('2'),
        "error message must name the valid range 1-2: {}",
        err.message
    );
}
