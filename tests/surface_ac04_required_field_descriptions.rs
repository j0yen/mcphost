//! PRD-mcphost-surface-fluidity AC4 — Given every tool descriptor, When the
//! schema test runs, Then every required property has a non-empty
//! description, including `host.bridge_test.spec` and
//! `host.spec_test.invocations`.
//!
//! Scope: the tenant-visible `host.*`/`billing.*`/`signup` surface built by
//! `handler.rs`'s `host_tools`/`signup_tool` (this PRD's engineering
//! target) -- not `admin.*`, an operator-only surface this PRD never
//! touches.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use serde_json::Value;

#[tokio::test]
async fn every_required_property_on_every_tenant_tool_has_a_description() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());

    let mut failures = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        let schema = &tool["inputSchema"];
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let properties = schema["properties"].as_object();
        for field in required {
            let description = properties
                .and_then(|p| p.get(field))
                .and_then(|p| p.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if description.is_empty() {
                failures.push(format!("{name}.{field}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "every required property must have a non-empty description, missing on: {failures:?}"
    );

    // The two properties the PRD's own audit named explicitly.
    let bridge_test = tools
        .iter()
        .find(|t| t["name"] == "host.bridge_test")
        .expect("host.bridge_test listed");
    let bridge_spec_desc = bridge_test["inputSchema"]["properties"]["spec"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        !bridge_spec_desc.is_empty(),
        "host.bridge_test.spec must have a description"
    );

    // host.spec_test only appears once authenticated -- already true here.
    let spec_test = tools
        .iter()
        .find(|t| t["name"] == "host.spec_test")
        .expect("host.spec_test listed for an authenticated tenant");
    let invocations_desc = spec_test["inputSchema"]["properties"]["invocations"]["description"]
        .as_str()
        .unwrap_or("");
    assert!(
        !invocations_desc.is_empty(),
        "host.spec_test.invocations must have a description"
    );
}
