//! PRD-mcphost-shared-tool-call-path
//! AC2 — Given O shares `lookup` to group G and B is not in G, When B calls
//! `host.tool_call {name: "<O>.lookup"}`, Then `tool_not_found` with the
//! hint text of requirement 2 returns.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

const EXPECTED_HINT: &str = "shared tools are called as <owner_namespace>.<tool>; ask the owner \
    to host.tool_share it with you or your group";

#[tokio::test]
async fn qualified_call_to_a_group_share_the_caller_is_not_in_returns_the_hint() {
    let server = TestServer::start().await;

    let (ns_o, key_o) = signup(&server.base_url, "Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("O publishes lookup");
    client_o
        .tools_call("host.group.create", json!({"name": "team"}))
        .await
        .expect("O creates group team");
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "lookup", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("O shares lookup to team");

    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let qualified = format!("{ns_o}.lookup");
    let err = client_b
        .tools_call("host.tool_call", json!({"name": qualified}))
        .await
        .expect_err("B is not in team, must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));
    assert_eq!(
        err.data["hint"].as_str(),
        Some(EXPECTED_HINT),
        "unexpected error data: {:?}",
        err.data
    );
}
