//! PRD-mcphost-surface-fluidity AC2 — Given `tools/list`, When the four
//! dry-run descriptions are read, Then each is at most 160 characters and
//! contains `host.quickstart`.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn every_dry_run_description_is_short_and_points_at_quickstart() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");

    for name in [
        "host.tool_test",
        "host.bridge_test",
        "host.spec_test",
        "host.tool_run",
    ] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} must be listed"));
        let description = tool["description"].as_str().expect("description string");
        assert!(
            description.len() <= 160,
            "{name}'s description must be at most 160 chars, got {}: {description}",
            description.len()
        );
        assert!(
            description.contains("host.quickstart"),
            "{name}'s description must mention host.quickstart: {description}"
        );
    }
}
