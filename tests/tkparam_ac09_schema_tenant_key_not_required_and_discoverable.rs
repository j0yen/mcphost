//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC9 (P2) — Given `tools/list` from an anonymous connection, When the
//! `tenant_key` property of each of the six tools' `inputSchema` is read,
//! Then `tenant_key` is absent from every `required` array, and its
//! `description` contains both `Authorization: Bearer` and
//! `tenant_key_missing`.

use crate::common;
use common::{McpClient, TestServer};

const SIX_TOOLS: &[&str] = &[
    "host.tool_publish",
    "host.catalog.search",
    "host.catalog.get",
    "host.state.table_create",
    "host.state.insert",
    "host.agent.profile_set",
];

#[tokio::test]
async fn tenant_key_is_optional_and_self_documenting_on_every_tool() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let listed = client.tools_list().await.expect("tools/list ok");
    let tools = listed["tools"].as_array().expect("tools array");

    for name in SIX_TOOLS {
        let tool = tools
            .iter()
            .find(|t| t["name"] == *name)
            .unwrap_or_else(|| panic!("{name} must be listed anonymously"));
        let schema = &tool["inputSchema"];

        let required = schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} inputSchema.required must be an array"));
        assert!(
            !required.iter().any(|v| v == "tenant_key"),
            "{name}: tenant_key must never be in required: {required:?}"
        );

        let description = schema["properties"]["tenant_key"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: tenant_key must have a description"));
        assert!(
            description.contains("Authorization: Bearer"),
            "{name}: tenant_key description must mention Authorization: Bearer: {description}"
        );
        assert!(
            description.contains("tenant_key_missing"),
            "{name}: tenant_key description must mention tenant_key_missing: {description}"
        );
    }
}
