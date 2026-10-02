//! PRD-mcphost-spec-unknown-field-rejection
//! AC5 — Given `tools/list`, When fetched, Then every `host.*` and
//! `billing.*` tool's `inputSchema.additionalProperties == false`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn every_host_and_billing_tool_schema_forbids_additional_properties() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list ok");
    let tools = listed["tools"].as_array().expect("tools array");

    let mut checked = 0;
    for tool in tools {
        let name = tool["name"].as_str().expect("tool name");
        if !(name.starts_with("host.") || name.starts_with("billing.")) {
            continue;
        }
        checked += 1;
        assert_eq!(
            tool["inputSchema"]["additionalProperties"],
            json!(false),
            "{name}'s inputSchema must set additionalProperties: false, got {:?}",
            tool["inputSchema"]
        );
    }
    assert!(
        checked > 100,
        "sanity: expected well over 100 host.*/billing.* tools, checked {checked}"
    );
}
