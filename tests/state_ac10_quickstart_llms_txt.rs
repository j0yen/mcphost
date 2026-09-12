//! PRD-mcphost-tenant-state
//! AC10 — Given `host.quickstart(kind="python")` and `llms.txt`, When read,
//! Then the quickstart has a state step and `llms.txt` lists every
//! `host.state.*` tool.

use crate::common;
use common::{TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const HOST_STATE_TOOLS: &[&str] = &[
    "host.state.get",
    "host.state.set",
    "host.state.delete",
    "host.state.list",
    "host.state.table_create",
    "host.state.table_drop",
    "host.state.query",
    "host.state.insert",
    "host.state.delete_rows",
];

#[tokio::test]
async fn quickstart_python_has_a_state_step() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "State AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&result);

    let steps = structured["steps"].as_array().expect("steps array");
    let state_step = steps
        .iter()
        .find(|s| s["call"].as_str().map(|c| c.starts_with("host.state.")).unwrap_or(false))
        .unwrap_or_else(|| panic!("quickstart(kind=python) must include a host.state.* step: {structured}"));
    assert!(
        state_step["note"]
            .as_str()
            .map(|n| n.to_lowercase().contains("state") || n.to_lowercase().contains("remember"))
            .unwrap_or(false),
        "state step should explain what it does: {state_step}"
    );

    // Plan limits (also part of AC10's requirement 6 wiring) name the state
    // quotas too.
    let limits = &structured["limits"]["plan"];
    assert!(limits["state_bytes_max"].is_number(), "limits.plan: {limits}");
    assert!(limits["state_rows_max"].is_number(), "limits.plan: {limits}");
    assert!(limits["state_ops_per_call_max"].is_number(), "limits.plan: {limits}");
}

#[test]
fn llms_txt_lists_every_host_state_tool() {
    for tool in HOST_STATE_TOOLS {
        assert!(
            LLMS_TXT.contains(tool),
            "www/llms.txt must list `{tool}` (run `mcphost llms-txt` to regenerate)"
        );
    }
}
