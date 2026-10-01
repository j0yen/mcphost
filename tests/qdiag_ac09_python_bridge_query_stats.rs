//! PRD-mcphost-query-diagnosis
//! AC9 (P1) — Given the python sandbox, When a tool calls
//! `mcphost.table.query_stats()`, Then it receives the same object the
//! tool returns.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_table_query_stats_matches_the_tool_from_sandboxed_code() {
    // Same skip-in-CI, fail-loudly-elsewhere convention every other
    // sandbox-dependent test in this crate uses.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "QDiag AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "t", "columns": {"x": "integer"}}))
        .await
        .expect("create");
    client
        .tools_call("host.table.query", json!({"sql": "SELECT * FROM t"}))
        .await
        .expect("seed query");

    let source = r#"import mcphost
def main(args):
    return mcphost.table.query_stats()
"#;
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stats_tool", "kind": "python", "spec": {"source": source}}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.stats_tool");
    let called = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("sandboxed query_stats call must succeed: {} {}", e.code, e.message));
    let from_sandbox = extract_structured(&called);
    let from_tool = extract_structured(
        &client.tools_call("host.table.query_stats", json!({})).await.expect("host.table.query_stats"),
    );

    assert_eq!(from_sandbox["queries"], from_tool["queries"], "sandbox: {from_sandbox}, tool: {from_tool}");
    assert_eq!(from_sandbox["ok"], from_tool["ok"], "sandbox: {from_sandbox}, tool: {from_tool}");
}
