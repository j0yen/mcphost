//! PRD-mcphost-result-handles
//! AC9 -- Given a `python` tool, When it calls
//! `mcphost.table.query(sql, handle=True)` then queries the handle, Then
//! both calls succeed inside the sandbox.
//!
//! Same shape `tests/tables_ac05_python_sandbox_table_access.rs` already
//! uses for `mcphost.table.create`/`.append`/`.query`: a real python-kind
//! tool, run through the real sandbox, this time exercising the
//! materialise-then-query-the-handle round trip requirement 8 adds to
//! `mcphost.table`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_table_query_handle_true_then_query_the_handle_succeed_from_sandboxed_tool_code() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Handle AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let source = r#"import mcphost
def main(args):
    mcphost.table.create(name="readings", columns={"sensor": "text", "value": "real"})
    mcphost.table.append(table="readings", rows=[
        {"sensor": "a", "value": 1.0},
        {"sensor": "b", "value": 2.0},
        {"sensor": "c", "value": 3.0},
    ])
    summary = mcphost.table.query(sql="SELECT * FROM readings", handle=True)
    handle = summary["handle"]
    result = mcphost.table.query(sql=f"SELECT COUNT(*) AS n FROM {handle}")
    handles = mcphost.table.handles()
    return {"row_count": summary["row_count"], "count": result["rows"][0]["n"], "handles": handles["handles"]}
"#;
    let spec = json!({"source": source});
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "handle_roundtrip", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.handle_roundtrip");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed handle materialise/query must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&called);

    assert_eq!(structured["row_count"], 3, "structured: {structured}");
    assert_eq!(structured["count"], 3, "structured: {structured}");
    assert_eq!(
        structured["handles"].as_array().expect("handles array").len(),
        1,
        "structured: {structured}"
    );
}
