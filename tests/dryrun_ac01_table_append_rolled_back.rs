//! PRD-mcphost-dry-run-side-effects
//! AC1 — Given a python tool that calls `mcphost.table.append` once, When
//! `host.tool_test` runs it, Then the result carries
//! `dry_run.writes == [{store: "table", op: "append", table: <t>, rows:
//! 1}]`, `rolled_back: true`, and `host.table.query` row count for `<t>`
//! is unchanged.
//!
//! Before this PRD, `host.tool_test` wired the real `TenantTableBridge`
//! straight into `CallCtx.table` (`src/handler.rs::tool_test`), so this
//! append committed for real -- the exact dogfood bug the PRD's Grounding
//! section names (`host.tool_test(fleet_ingest_row, ...)` leaving a real
//! row behind). This test proves the fix: the row never lands, and the
//! call's own result says so.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_test_table_append_reports_dry_run_and_leaves_no_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dryrun AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The table must already exist -- the AC's own precondition is a tool
    // that appends, not one that creates.
    client
        .tools_call(
            "host.table.create",
            json!({"name": "ingest", "columns": {"n": "integer"}}),
        )
        .await
        .expect("host.table.create ok");

    let source = "import mcphost\ndef main(args):\n    mcphost.table.append(table=\"ingest\", rows=[{\"n\": 1}])\n    return {\"ok\": True}\n";
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "appender", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    let tested = poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "appender", "args": {}}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&tested);
    let dry_run = &structured["dry_run"];
    assert_eq!(dry_run["delivered"], json!(false), "{structured}");
    assert_eq!(dry_run["rolled_back"], json!(true), "{structured}");
    let writes = dry_run["writes"].as_array().unwrap_or_else(|| panic!("writes array: {structured}"));
    assert_eq!(writes.len(), 1, "exactly one write: {structured}");
    assert_eq!(writes[0]["store"], json!("table"));
    assert_eq!(writes[0]["op"], json!("append"));
    assert_eq!(writes[0]["table"], json!("ingest"));
    assert_eq!(writes[0]["rows"], json!(1));

    // The real row count is unchanged -- a real `host.table.query` (never
    // a dry run) proves it.
    let queried = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM ingest"}))
            .await
            .expect("host.table.query ok"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert!(
        rows.is_empty(),
        "host.tool_test's append must not have persisted a real row: {rows:?}"
    );
}
