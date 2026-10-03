//! PRD-mcphost-dry-run-side-effects
//! AC7 — Given a tenant one row below `table_rows_max`, When `host.tool_test`
//! runs a tool that appends two rows, Then the call fails with the real
//! quota error class and the table is unchanged.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{json, Value};

#[tokio::test]
async fn tool_test_still_enforces_the_real_quota_and_rolls_back() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dry Run AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "runs", "columns": {"slug": "text"}}),
        )
        .await
        .expect("table create ok");

    // The free plan's table_rows_max is 5,000 (src/plans.rs) -- one bulk
    // append gets the table to exactly one row below it.
    let table_rows_max: i64 = 5_000;
    let rows: Vec<Value> = (0..table_rows_max - 1)
        .map(|i| json!({"slug": format!("row-{i}")}))
        .collect();
    client
        .tools_call("host.table.append", json!({"table": "runs", "rows": rows}))
        .await
        .expect("bulk append ok");

    let source = r#"import mcphost
def main(args):
    mcphost.table.append(table="runs", rows=[{"slug": "a"}, {"slug": "b"}])
    return {"ok": True}
"#;
    publish(&client, "overflow", "python", json!({"source": source})).await;

    let err = client
        .tools_call("host.tool_test", json!({"name": "overflow", "args": {}}))
        .await
        .expect_err("appending past table_rows_max must fail even under a dry run");
    // The quota rejection happens inside `mcphost.table.append`, inside the
    // tool's own python code -- same as any other uncaught exception
    // escaping the tool, it surfaces as `tool_exception` (PRD-mcphost-
    // python-kind-runtime requirement 1/AC2), never silently swallowed or
    // downgraded into a generic failure: the real `table_rows_max` quota
    // rejection is right there in the message and exception class.
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    assert_eq!(err.data["exception_class"], json!("McphostTableError"), "{err:?}");
    assert!(
        err.message.contains("table_rows_max"),
        "the real quota class must be visible, not swallowed: {err:?}"
    );

    let queried = client
        .tools_call("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM runs"}))
        .await
        .expect("table query ok");
    let rows = extract_structured(&queried)["rows"].as_array().expect("rows array").clone();
    assert_eq!(
        rows[0]["n"],
        json!(table_rows_max - 1),
        "a failed dry run must leave the table exactly as it was: {rows:?}"
    );
}
