//! PRD-mcphost-result-handles
//! AC9 (P1) -- Given a `python` tool, When it calls
//! `mcphost.table.query(sql, handle=True)` then queries the handle, Then
//! both calls succeed inside the sandbox.
//!
//! Same shape `tables_ac05_python_sandbox_table_access.rs` already uses
//! for the ordinary `mcphost.table.create`/`.append`/`.query` path: a real
//! python-kind tool, run through the real sandbox, over the
//! `TableSidecarBridge` request/response channel -- here exercising the
//! `handle=True` kwarg `_table_query` (PRD-mcphost-result-handles P1
//! requirement 8) added to `kinds::python::PY_RUNNER_SCRIPT`'s
//! `mcphost.table` module.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_table_query_handle_true_then_query_handle_both_succeed_from_sandbox() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "ResultHandles AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let source = r#"import mcphost
def main(args):
    mcphost.table.create(name="readings", columns={"sensor": "text", "value": "real"})
    mcphost.table.append(table="readings", rows=[
        {"sensor": "a", "value": 1.0},
        {"sensor": "b", "value": 2.0},
        {"sensor": "c", "value": 3.0},
    ])
    materialized = mcphost.table.query(sql="SELECT sensor, value FROM readings", handle=True, ttl_s=60)
    handle = materialized["handle"]
    queried = mcphost.table.query(sql=f"SELECT COUNT(*) AS n FROM {handle}")
    handles_listed = mcphost.table.handles()
    return {
        "row_count": materialized["row_count"],
        "count_from_handle": queried["rows"][0]["n"],
        "handles_count": len(handles_listed["handles"]),
    }
"#;
    let spec = json!({"source": source});
    client
        .tools_call("host.tool_publish", json!({"name": "handle_roundtrip", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.handle_roundtrip");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed handle round trip must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&called);
    assert_eq!(structured["row_count"], 3, "structured: {structured}");
    assert_eq!(structured["count_from_handle"], 3, "structured: {structured}");
    assert_eq!(structured["handles_count"], 1, "structured: {structured}");
}
