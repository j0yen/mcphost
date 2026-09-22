//! PRD-mcphost-tool-versions
//! AC8 (P1) — Given `host.tool_remove`, When called, Then all versions are
//! gone and `host.tool_history` returns not-found.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn remove_clears_all_versions_and_history_is_not_found() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 1..=3 {
        client
            .tools_call(
                "host.tool_publish",
                json!({
                    "name": "gone_soon",
                    "kind": "echo",
                    "spec": {"schema": {"type": "object", "properties": {"f": {"type": "string", "enum": [format!("v{i}")]}}, "required": ["f"]}},
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("publish v{i} should succeed: {} {}", e.code, e.message));
    }

    client
        .tools_call("host.tool_remove", json!({"name": "gone_soon"}))
        .await
        .expect("remove should succeed");

    let err = client
        .tools_call("host.tool_history", json!({"name": "gone_soon"}))
        .await
        .expect_err("tool_history on a removed tool must fail");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));

    // Republishing under the same name must start a fresh version 1 -- no
    // orphaned tool_versions rows from before the remove survived it.
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "gone_soon",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
            }),
        )
        .await
        .expect("republish under the same name should succeed");
    let history = client
        .tools_call("host.tool_history", json!({"name": "gone_soon"}))
        .await
        .expect("tool_history should succeed again after republish");
    let versions = extract_structured(&history)["versions"].clone();
    let versions = versions.as_array().expect("versions array");
    assert_eq!(versions.len(), 1, "only the fresh republish's version should exist: {versions:?}");
    assert_eq!(versions[0]["version"], json!(1));
}
