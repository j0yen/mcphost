//! PRD-mcphost-tenant-tables
//! AC8 — Given the build completes, When llms.txt and descriptors are
//! read, Then all table tools and the KV-vs-table sentence are present.
//!
//! `tests/surface_ac05_llms_txt_tool_parity.rs` already proves every live
//! tool (including the six `host.table.*` ones) is listed in `www/llms.txt`
//! generically; this test is AC8's own dedicated, self-contained proof --
//! it names the six tools directly (same convention as
//! `tests/state_ac10_quickstart_llms_txt.rs` for `host.state.*`) and, the
//! part nothing else checked, that requirement 6's "one sentence on
//! table-store vs key-value store choice" actually exists somewhere an
//! agent discovers it (`host.quickstart`'s `host.state.set` step note).

use crate::common;
use common::{TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const HOST_TABLE_TOOLS: &[&str] = &[
    "host.table.create",
    "host.table.append",
    "host.table.query",
    "host.table.list",
    "host.table.drop",
    "host.table.schema",
];

#[test]
fn llms_txt_lists_every_host_table_tool() {
    for tool in HOST_TABLE_TOOLS {
        assert!(
            LLMS_TXT.contains(tool),
            "www/llms.txt must list `{tool}` (run `mcphost llms-txt` to regenerate)"
        );
    }
}

#[tokio::test]
async fn quickstart_state_step_names_the_kv_vs_table_choice() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Table AC8 Tenant").await;
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
    let note = state_step["note"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        note.contains("host.table") && note.contains("host.state"),
        "quickstart's host.state step must name the table-store vs key-value store choice \
         (requirement 6), mentioning both host.state and host.table: {state_step}"
    );

    // Plan limits also name the table quotas, same wiring AC4 exercises
    // from the session side.
    let limits = &structured["limits"]["plan"];
    assert!(limits["table_tables_max"].is_number(), "limits.plan: {limits}");
    assert!(limits["table_rows_max"].is_number(), "limits.plan: {limits}");
    assert!(limits["table_bytes_max"].is_number(), "limits.plan: {limits}");
}
