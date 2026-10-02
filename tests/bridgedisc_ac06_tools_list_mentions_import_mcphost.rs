//! PRD-mcphost-sandbox-bridge-discoverability
//! AC6 (P1) -- Given `tools/list`, When fetched, Then the
//! `host.tool_publish` description and the python kind's field help each
//! contain `import mcphost` and `mcphost.table`.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

fn assert_mentions_sandbox_api(text: &str, label: &str) {
    assert!(text.contains("import mcphost"), "{label} must contain 'import mcphost': {text}");
    assert!(text.contains("mcphost.table"), "{label} must contain 'mcphost.table': {text}");
}

#[tokio::test]
async fn tool_publish_description_and_spec_field_help_mention_the_sandbox_api() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let publish = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed");

    let description = publish["description"].as_str().expect("description is a string");
    assert_mentions_sandbox_api(description, "host.tool_publish's description");
    assert!(
        description.len() <= 600,
        "host.tool_publish's description must stay at most 600 chars (surface_ac07): {}",
        description.len()
    );

    // The python kind's own field help (kinds::python::python_field_hint's
    // "network" entry) -- embedded into the spec property's description.
    let spec_description = publish["inputSchema"]["properties"]["spec"]["description"]
        .as_str()
        .expect("spec property description is a string");
    assert_mentions_sandbox_api(spec_description, "the python kind's field help");
}
