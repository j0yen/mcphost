//! PRD-mcphost-dry-run-side-effects
//! AC6 — Given `tools/list`, When fetched, Then `host.tool_test` and
//! `host.trigger.test` descriptions contain "rolled back" and "dry_run".
//!
//! `host.tool_test`'s description is also covered by the 160-char/
//! `host.quickstart` length constraint `tests/surface_ac02_dry_run_
//! descriptions.rs` already enforces -- this test adds the two words
//! this PRD's requirement 5 specifically promises, on both entry points.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tool_test_and_trigger_test_descriptions_say_rolled_back_and_dry_run() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryrun AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

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
            "{name}'s description must say 'rolled back': {description}"
        );
        assert!(
            description.contains("dry_run"),
            "{name}'s description must say 'dry_run': {description}"
        );
    }
}
