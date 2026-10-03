//! PRD-mcphost-dry-run-side-effects
//! AC6 — Given `tools/list`, When fetched, Then `host.tool_test` and
//! `host.trigger.test` descriptions contain "rolled back" and "dry_run".

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tool_test_and_trigger_test_descriptions_name_rollback_and_dry_run() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC6 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");

    for name in ["host.tool_test", "host.trigger.test"] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name} must be listed"));
        let description = tool["description"].as_str().expect("description string");
        assert!(
            description.contains("rolled back"),
            "{name}'s description must mention \"rolled back\": {description}"
        );
        assert!(
            description.contains("dry_run"),
            "{name}'s description must mention \"dry_run\": {description}"
        );
    }
}
