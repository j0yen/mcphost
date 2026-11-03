//! AC1 (PRD-mcphost-admin-signup-pause-tool) — Given a test server with the
//! admin key, When the admin lists tools, Then `admin.signup_pause` is listed
//! with `message` (string), `resume` (boolean) and `until` (integer)
//! properties, its description says exactly one of `message` or `resume: true`
//! is required, every descriptor is a dispatch arm and every arm a descriptor
//! (the adisp drift test), and README.md § signup names the tool.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::Value;

#[tokio::test]
async fn aspt_ac01_admin_lists_signup_pause_with_its_schema() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listed = admin.tools_list().await.expect("admin tools/list");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "admin.signup_pause")
        .unwrap_or_else(|| panic!("admin.signup_pause not listed: {listed:?}"));
    let props = &tool["inputSchema"]["properties"];
    assert_eq!(props["message"]["type"], "string", "{tool:?}");
    assert_eq!(props["resume"]["type"], "boolean", "{tool:?}");
    assert_eq!(props["until"]["type"], "integer", "{tool:?}");
    let required = tool["inputSchema"].get("required").and_then(Value::as_array);
    assert!(required.is_none_or(|r| r.is_empty()), "no required entry: {tool:?}");
    let description = tool["description"].as_str().expect("description");
    assert!(
        description.contains("Exactly one of `message` or `resume: true` is required"),
        "{description}"
    );
}

#[test]
fn aspt_ac01_readme_signup_section_names_the_tool() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).expect("read README.md");
    let at = readme.find("signup_paused").expect("README names signup_paused");
    let section = &readme[at..(at + 800).min(readme.len())];
    assert!(section.contains("admin.signup_pause"), "README § signup must name the tool: {section}");
}
