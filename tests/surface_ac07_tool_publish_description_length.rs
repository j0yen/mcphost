//! PRD-mcphost-surface-fluidity AC7 (P1, requirement 6) — Given
//! `host.tool_publish`'s description, When measured, Then it is at most
//! 600 characters. The per-kind example specs the older
//! PRD-mcphost-publish-first-try design embedded here now live in
//! `host.quickstart(kind)` (already the case before this PRD -- see
//! `control::quickstart`'s `steps`), so this description only needs to
//! name the registered kinds and point at `host.quickstart` for detail.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn tool_publish_description_is_at_most_600_chars() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    let publish = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_publish"))
        .expect("host.tool_publish is listed");
    let description = publish["description"]
        .as_str()
        .expect("description is a string");

    assert!(
        description.len() <= 600,
        "host.tool_publish's description must be at most 600 chars, got {}: {description}",
        description.len()
    );
}
