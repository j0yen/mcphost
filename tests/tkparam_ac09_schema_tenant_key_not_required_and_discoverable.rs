//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC9 (P2) — Given `tools/list` from an anonymous connection, When the
//! `tenant_key` property of each of the six tools' `inputSchema` is read,
//! Then `tenant_key` is absent from every `required` array, and its
//! `description` contains both `Authorization: Bearer` and a statement of
//! what omitting it does.
//!
//! PRD-mcphost-one-next-tool requirement 1 narrowed an anonymous
//! `tools/list` to a twelve-tool starter set that excludes four of these
//! six (`host.catalog.search`/`get`, `host.state.table_create`/`insert`,
//! `host.agent.profile_set`) -- switched to an authenticated session per
//! that PRD's own migration note ("switched to authenticated sessions"):
//! `host_schema`'s generated `tenant_key` property (what this test actually
//! proves) is identical regardless of auth state.
//!
//! PRD-mcphost-implicit-signup requirement 5: omitting `tenant_key` on a
//! connection with no header and no prior signup no longer returns
//! `tenant_key_missing` -- it implicitly signs that connection up
//! (requirement 1), and the description now says that instead.

use crate::common;
use common::{McpClient, TestServer, signup};

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
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

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
            description.contains("implicitly signs that connection up"),
            "{name}: tenant_key description must state what omitting it on a bare \
             connection now does: {description}"
        );
    }
}
