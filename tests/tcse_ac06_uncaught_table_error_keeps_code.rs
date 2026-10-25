//! PRD-mcphost-table-conflict-structured-error
//! AC6 -- Given the AC5 tool without the `try`, When run via
//! `host.tool_call`, Then the run error's code is `table_row_conflict`,
//! `data.phase == "tool_code"`, `data.exception_class == "McphostTableError"`,
//! and the error is not `-32603`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, publish, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn tcse_ac06() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Tcse AC6 Tenant").await;
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
    mcphost.table.append(table="events", rows=[{"seq": 1}])
    return {"ok": True}
"#;
    publish(&client, "dup", "python", json!({"source": source})).await;

    let err = client
        .tools_call("host.tool_call", json!({"name": "dup", "args": {}}))
        .await
        .expect_err("uncaught duplicate must fail the run");
    assert_eq!(err.error_code.as_deref(), Some("table_row_conflict"), "{err:?}");
    assert_eq!(err.data["phase"], json!("tool_code"), "{err:?}");
    assert_eq!(err.data["exception_class"], json!("McphostTableError"), "{err:?}");
    assert_ne!(err.code, -32603, "{err:?}");
}
