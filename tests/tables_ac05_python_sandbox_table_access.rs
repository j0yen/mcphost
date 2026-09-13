//! PRD-mcphost-tenant-tables
//! AC5 — Given a python tool whose code appends and queries via the
//! sandbox channel, When called, Then the operations succeed under the
//! same tenant scoping and quotas as the session tools.
//!
//! This is the `mcphost.table` counterpart of
//! `tests/state_ac02_ac03_python_sandbox.rs` (which proves the same shape
//! for `mcphost.state`): a real python-kind tool, run through the real
//! sandbox, calling `mcphost.table.create`/`.append`/`.query` over the
//! `TableSidecarBridge` request/response channel
//! (`kinds::python::PY_RUNNER_SCRIPT`'s `mcphost.table` module, host side
//! in `kinds::python`'s sidecar handling) rather than exercising
//! `tables::table_create`/`table_append`/`table_query` directly as
//! `src/tables.rs`'s own unit tests do. Nothing in `src/tables.rs`'s
//! `#[cfg(test)]` module goes through the sandbox at all, so AC5's actual
//! claim -- that tool code, not just the agent session, can reach the
//! table store -- was previously untested.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_table_create_append_query_succeed_from_sandboxed_tool_code() {
    // Same skip-in-CI, fail-loudly-elsewhere convention every other
    // sandbox-dependent test in this crate uses (see
    // `tests/python_ac01_no_deps_cpu_memory.rs`).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Table AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The tool's own code reaches the real-SQL store through
    // `mcphost.table`, not `mcphost.state` -- create a table, append rows,
    // and run a read-only SQL query, entirely from inside the sandbox.
    let source = r#"import mcphost
def main(args):
    mcphost.table.create(name="readings", columns={"sensor": "text", "value": "real"})
    mcphost.table.append(table="readings", rows=[
        {"sensor": "a", "value": 1.0},
        {"sensor": "b", "value": 2.0},
        {"sensor": "c", "value": 3.0},
    ])
    result = mcphost.table.query(sql="SELECT sensor, value FROM readings WHERE value > 1.5 ORDER BY value")
    return {"rows": result["rows"]}
"#;
    let spec = json!({"source": source});
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "sensor_ingest", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.sensor_ingest");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed table call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&called);
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2, "only the two rows with value > 1.5 must return: {rows:?}");
    assert_eq!(rows[0]["sensor"], json!("b"));
    assert_eq!(rows[1]["sensor"], json!("c"));

    // Same tenant scoping as the session tools: the table the sandboxed
    // tool created is visible to `host.table.*` under the same tenant.
    let listed = extract_structured(
        &client
            .tools_call("host.table.list", json!({}))
            .await
            .expect("host.table.list"),
    );
    let names: Vec<&str> = listed["tables"]
        .as_array()
        .expect("tables array")
        .iter()
        .map(|t| t["name"].as_str().expect("table name"))
        .collect();
    assert!(
        names.contains(&"readings"),
        "the table the sandboxed tool created must be visible to host.table.list: {names:?}"
    );
}
