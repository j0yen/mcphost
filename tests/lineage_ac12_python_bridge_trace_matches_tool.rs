//! PRD-mcphost-lineage-blast-radius AC12 (P1) -- Given a `python` tool, When it calls
//! `mcphost.lineage.trace("table:orders")`, Then it receives the same
//! object the tool returns.

use crate::common;
use common::{TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::lineage::{self, NodeKind};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn python_bridge_trace_matches_the_hosts_own_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Lineage AC12 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("db")
        .expect("tenant exists");

    lineage::register_edge(
        &server.state,
        tenant.id,
        (NodeKind::Table, "orders", "orders"),
        (NodeKind::Tool, "orders_reader", "orders_reader"),
        "declared_reads",
    )
    .await
    .expect("register edge");

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "trace_bridge",
                "kind": "python",
                "spec": {"source": "import mcphost\ndef main(args):\n    return mcphost.lineage.trace(\"table:orders\")\n"},
            }),
        )
        .await
        .expect("publish trace_bridge");

    let via_tool = extract_structured(
        &client
            .tools_call(&format!("{ns}.trace_bridge"), json!({}))
            .await
            .expect("call trace_bridge"),
    );

    let via_host_tool = extract_structured(
        &client
            .tools_call("host.lineage.trace", json!({"id": "table:orders"}))
            .await
            .expect("host.lineage.trace"),
    );

    assert_eq!(via_tool, via_host_tool, "the python bridge must return the exact same object host.lineage.trace does");
}
