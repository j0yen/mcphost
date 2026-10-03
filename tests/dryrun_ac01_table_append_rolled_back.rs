//! PRD-mcphost-dry-run-side-effects
//! AC1 — Given a python tool that calls `mcphost.table.append` once, When
//! `host.tool_test` runs it, Then the result carries `dry_run.writes ==
//! [{store: "table", op: "append", table: <t>, rows: 1}]`, `rolled_back:
//! true`, and `host.table.query` row count for `<t>` is unchanged.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tool_test_on_table_append_rolls_back_and_reports_the_write() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC1 Tenant").await;
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
    mcphost.table.append(table="runs", rows=[{"slug": "pyingest-dryrun"}])
    return {"ok": True}
"#;
    publish(&client, "ingest", "python", json!({"source": source})).await;

    let result = client
        .tools_call("host.tool_test", json!({"name": "ingest", "args": {}}))
        .await
        .expect("tool_test ok");
    let structured = extract_structured(&result);

    assert_eq!(
        structured["dry_run"]["writes"],
        json!([{"store": "table", "op": "append", "table": "runs", "rows": 1}]),
        "{structured:?}"
    );
    assert_eq!(structured["dry_run"]["rolled_back"], json!(true), "{structured:?}");
    assert_eq!(structured["dry_run"]["delivered"], json!(false), "{structured:?}");

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
