//! PRD-mcphost-dry-run-side-effects
//! AC2 — Given a python tool that appends a row and then queries the same
//! table in the same call, When run under `host.tool_test`, Then the
//! tool's own result shows the appended row (visible inside the
//! savepoint) and the table is unchanged afterward.
//!
//! This is the requirement-2 proof that `TestTableBridge` holds ONE
//! connection (and one `SAVEPOINT`) for the whole call rather than opening
//! a fresh connection per op: an append followed by a query in the SAME
//! `Kind::call` must see the uncommitted write, even though nothing ever
//! reaches disk.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tool_test_append_then_query_sees_uncommitted_row_but_table_stays_empty() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dryrun AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "readings", "columns": {"sensor": "text"}}),
        )
        .await
        .expect("host.table.create ok");

    let source = "import mcphost\ndef main(args):\n    mcphost.table.append(table=\"readings\", rows=[{\"sensor\": \"a\"}])\n    result = mcphost.table.query(sql=\"SELECT sensor FROM readings\")\n    return {\"rows\": result[\"rows\"]}\n";
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "append_then_query", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    let tested = poll_until_ready(
        &client,
        "host.tool_test",
        json!({"name": "append_then_query", "args": {}}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("host.tool_test must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&tested);

    // The tool's OWN result, from inside the savepoint, shows the row it
    // just appended.
    // `host.tool_test`'s python echo nests the tool's own return value
    // under `result` (see `PythonKind::call`'s `ctx.test_mode` branch).
    let rows = structured["result"]["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("rows array: {structured}"));
    assert_eq!(rows.len(), 1, "the query inside the same call must see the uncommitted append: {structured}");
    assert_eq!(rows[0]["sensor"], json!("a"));

    // `dry_run.writes` names exactly the one append (the query is a read,
    // not a write).
    let writes = structured["dry_run"]["writes"]
        .as_array()
        .unwrap_or_else(|| panic!("writes array: {structured}"));
    assert_eq!(writes.len(), 1, "{structured}");
    assert_eq!(writes[0]["op"], json!("append"));

    // After the call ends, a real query shows the table unchanged.
    let queried = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM readings"}))
            .await
            .expect("host.table.query ok"),
    );
    assert!(
        queried["rows"].as_array().expect("rows array").is_empty(),
        "the appended row must not survive past the dry run: {queried}"
    );
}
