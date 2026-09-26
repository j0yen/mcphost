//! PRD-mcphost-shared-tool-call-path
//! AC8 — Given B has one direct share and one group share, When B calls
//! host.get_info, Then shared_tools lists both with via set correctly.
//!
//! The PRD names this call `host.get_info`, but this crate has no tool by
//! that literal name -- `host.whoami` is its real "tell the caller about
//! itself" tool (the only one with tenant context at all; the MCP
//! protocol-level `get_info`/`initialize` response runs before
//! authentication and could never carry a per-caller field). See
//! `Db::shared_tools_for_caller`'s doc comment for the same reasoning.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_lists_direct_and_group_shares_with_via() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "Tenant A").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "geo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("A publishes geo");
    client_a
        .tools_call("host.tool_share", json!({"name": "geo", "visibility": "public"}))
        .await
        .expect("A shares geo publicly");

    let (ns_c, key_c) = signup(&server.base_url, "Tenant C").await;
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);
    client_c
        .tools_call(
            "host.tool_publish",
            json!({"name": "memory", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("C publishes memory");
    client_c
        .tools_call("host.group.create", json!({"name": "team"}))
        .await
        .expect("C creates group team");

    let (ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    client_c
        .tools_call("host.group.add", json!({"name": "team", "namespace": ns_b}))
        .await
        .expect("C adds B to team");
    client_c
        .tools_call(
            "host.tool_share",
            json!({"name": "memory", "visibility": "group", "group": "team"}),
        )
        .await
        .expect("C shares memory to team");

    let whoami = client_b
        .tools_call("host.whoami", json!({}))
        .await
        .expect("B calls host.whoami");
    let structured = extract_structured(&whoami);
    let shared_tools = structured["shared_tools"].as_array().expect("shared_tools array");

    let direct = shared_tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns_a}.geo")))
        .unwrap_or_else(|| panic!("shared_tools must list {ns_a}.geo: {shared_tools:?}"));
    assert_eq!(direct["owner"], json!(ns_a));
    assert_eq!(direct["via"], json!("direct"));

    let via_group = shared_tools
        .iter()
        .find(|t| t["name"] == json!(format!("{ns_c}.memory")))
        .unwrap_or_else(|| panic!("shared_tools must list {ns_c}.memory: {shared_tools:?}"));
    assert_eq!(via_group["owner"], json!(ns_c));
    assert_eq!(via_group["via"], json!("group:team"));
}
