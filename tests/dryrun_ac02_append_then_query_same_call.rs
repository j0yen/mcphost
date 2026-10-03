//! PRD-mcphost-dry-run-side-effects
//! AC2 — Given a python tool that appends a row and then queries the same
//! table in the same call, When run under `host.tool_test`, Then the
//! tool's own result shows the appended row (visible inside the
//! savepoint) and the table is unchanged afterwards.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tool_test_sees_its_own_uncommitted_append_but_leaves_the_table_unchanged() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"slug": "text"}}),
        )
        .await
        .expect("table create ok");

    let source = r#"import mcphost
def main(args):
    mcphost.table.append(table="runs", rows=[{"slug": "x"}])
    result = mcphost.table.query(sql="SELECT COUNT(*) AS n FROM runs")
    return {"n": result["rows"][0]["n"]}
"#;
    publish(&client, "ingest", "python", json!({"source": source})).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "ingest", "args": {}}))
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    // Visible inside the savepoint: the tool's own query sees the row it
    // just appended, in the same call.
    assert_eq!(structured["result"]["n"], json!(1), "{structured:?}");
    assert_eq!(
        structured["dry_run"]["writes"],
        json!([{"store": "table", "op": "append", "table": "runs", "rows": 1}]),
        "{structured:?}"
    );

    // Gone afterward: a fresh call (its own connection) sees none of it.
    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM runs"}))
        .await
        .expect("table query ok");
    let rows = extract_structured(&queried)["rows"].as_array().expect("rows array").clone();
    assert_eq!(
        rows[0]["n"],
        json!(0),
        "a dry run must not persist the appended row: {rows:?}"
    );
}
