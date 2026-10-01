//! PRD-mcphost-one-next-tool AC1 (P0) — Given a fresh anonymous session on
//! `/mcp`, When `tools/list` is called, Then exactly the 12 starter tools
//! are returned and `signup` is first.

use crate::common;
use common::{McpClient, TestServer};

const EXPECTED_STARTER_SET: [&str; 12] = [
    "signup",
    "host.quickstart",
    "host.redeem",
    "billing.plans",
    "host.whoami",
    "host.tool_publish",
    "host.tool_call",
    "host.tool_test",
    "host.state.set",
    "host.state.get",
    "host.state.list",
    "host.tool_share",
];

#[tokio::test]
async fn anonymous_tools_list_is_exactly_the_starter_set_signup_first() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client
        .tools_list()
        .await
        .expect("tools/list should succeed unauthenticated");
    let tool_names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    assert_eq!(
        tool_names.first(),
        Some(&"signup"),
        "signup must be first: {tool_names:?}"
    );
    assert_eq!(
        tool_names.len(),
        12,
        "exactly the 12 starter tools, nothing more: {tool_names:?}"
    );
    for expected in EXPECTED_STARTER_SET {
        assert!(
            tool_names.contains(&expected),
            "starter set must contain {expected}: {tool_names:?}"
        );
    }
}
