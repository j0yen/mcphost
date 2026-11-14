//! PRD-mcphost-table-sql-exec-and-grammar
//! AC7 -- Given a python tool calling `mcphost.table.exec("INSERT INTO t (id,a) VALUES (9,1)")`,
//! When called via `host.tool_call`, Then the row exists and `host.table.query_log` shows
//! `statement_kind: insert, changed: 1`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_python_tool_writes_a_row_with_mcphost_table_exec_and_the_query_log_records_it() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Tsql AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer", "a": "integer"}, "primary_key": "id"}))
        .await
        .expect("create t");
    client
        .tools_call("host.table.append", json!({"table": "t", "rows": [{"id": 1, "a": 1}, {"id": 2, "a": 5}]}))
        .await
        .expect("seed rows");

    let source = r#"import mcphost
def main(args):
    written = mcphost.table.exec("INSERT INTO t (id,a) VALUES (9,1)")
    found = mcphost.table.find("t", {"a": {">": 3}})
    try:
        mcphost.table.exec("DELETE FROM t")
        refused = None
    except mcphost.table.TableError as e:
        refused = e.code
    return {"written": written, "found": [r["id"] for r in found["rows"]], "refused": refused}
"#;
    publish(&client, "writer", "python", json!({"source": source})).await;

    let called = poll_until_ready(&client, "host.tool_call", json!({"name": "writer", "args": {}}), Duration::from_secs(20))
        .await
        .unwrap_or_else(|e| panic!("the python tool must run: {} {}", e.code, e.message));
    let structured = extract_structured(&called);
    let result = structured.get("result").unwrap_or(&structured);
    assert_eq!(result["written"], json!({"changed": 1, "table": "t"}), "{structured}");
    assert_eq!(result["found"], json!([2]), "mcphost.table.find reads through the bridge: {structured}");
    assert_eq!(result["refused"], json!("table_exec_rejected"), "a refusal reaches the tool as a TableError: {structured}");

    let row = extract_structured(&client.tools_call("host.table.query", json!({"sql": "SELECT a FROM t WHERE id = 9"})).await.expect("read back"));
    assert_eq!(row["rows"], json!([{"a": 1}]), "the row exists");

    let log = extract_structured(&client.tools_call("host.table.query_log", json!({"limit": 50})).await.expect("query_log"));
    let rows = log["rows"].as_array().expect("rows");
    let insert = rows
        .iter()
        .find(|r| r["sql"].as_str().is_some_and(|s| s.starts_with("INSERT INTO t")))
        .unwrap_or_else(|| panic!("the exec INSERT is in the query log: {log}"));
    assert_eq!(insert["statement_kind"], "insert", "{insert}");
    assert_eq!(insert["changed"], 1, "{insert}");
    assert!(insert["error_code"].is_null() && insert["row_count"].is_null(), "an exec row reports changed instead of row_count: {insert}");

    let refused = rows.iter().find(|r| r["sql"] == "DELETE FROM t").unwrap_or_else(|| panic!("the refused DELETE is logged: {log}"));
    assert_eq!(refused["statement_kind"], "delete", "{refused}");
    assert_eq!(refused["error_code"], "table_exec_rejected", "{refused}");
    assert!(refused["changed"].is_null(), "{refused}");

    let select = rows.iter().find(|r| r["sql"].as_str().is_some_and(|s| s.starts_with("SELECT") && s.contains("id = 9"))).expect("the read-back SELECT is logged");
    assert_eq!(select["statement_kind"], "select", "{select}");
    assert!(select["changed"].is_null(), "{select}");
}
