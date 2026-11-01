//! PRD-mcphost-table-conflict-structured-error
//! AC5 -- Given a python tool calling `mcphost.table.append` on the AC1 table
//! with a duplicate `seq` inside `try/except mcphost.table.TableError as e`,
//! When run, Then `e.code == "table_row_conflict"` and `e.data["column"] == "seq"`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tcse_ac05() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Tcse AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {"seq": "integer"}, "primary_key": "seq"}),
        )
        .await
        .expect("table create ok");
    client
        .tools_call("host.table.append", json!({"table": "events", "rows": [{"seq": 1}]}))
        .await
        .expect("first append ok");

    let source = r#"import mcphost
def main(args):
    try:
        mcphost.table.append(table="events", rows=[{"seq": 1}])
    except mcphost.table.TableError as e:
        return {"code": e.code, "column": e.data["column"]}
    return {"code": None}
"#;
    publish(&client, "dup", "python", json!({"source": source})).await;

    let result = client
        .tools_call("host.tool_call", json!({"name": "dup", "args": {}}))
        .await
        .expect("tool call ok");
    let structured = extract_structured(&result);
    let out = structured.get("result").unwrap_or(&structured);
    assert_eq!(out["code"], json!("table_row_conflict"), "{structured:?}");
    assert_eq!(out["column"], json!("seq"), "{structured:?}");
}
