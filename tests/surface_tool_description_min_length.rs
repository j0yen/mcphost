//! Regression guard for the mcphost.dev promise-ledger finding (2026-10-01):
//! `host.agent.unmute`/`host.msg.unblock` carried descriptions under 20
//! characters ("Remove a mute."/"Remove a block.") -- too short to tell a
//! client what the tool actually does. Every tenant-visible tool's own
//! top-level description (not a property description -- see
//! `surface_ac04_required_field_descriptions.rs` for those) must be at
//! least 20 characters.
//!
//! Scope: the tenant-visible `host.*`/`billing.*`/`signup` surface built by
//! `handler.rs`'s `host_tools`/`signup_tool` -- not `admin.*`, an
//! operator-only surface this test never touches.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};

const MIN_DESCRIPTION_LEN: usize = 20;

#[tokio::test]
async fn every_tenant_tool_description_is_at_least_20_chars() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Surface Desc-Len Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client.tools_list().await.expect("tools/list");
    let tools = result["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty());

    let mut failures = Vec::new();
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        let description = tool["description"].as_str().unwrap_or("");
        if description.len() < MIN_DESCRIPTION_LEN {
            failures.push(format!(
                "{name} ({} chars: {description:?})",
                description.len()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "every tenant-visible tool must have a description of at least \
         {MIN_DESCRIPTION_LEN} characters, too short on: {failures:?}"
    );
}
