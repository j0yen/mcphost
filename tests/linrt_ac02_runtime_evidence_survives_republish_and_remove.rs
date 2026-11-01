//! PRD-mcphost-lineage-runtime-edge-writers AC2 (P0) -- Given tool `a` (published as `tool_a`: tool names need two
//! characters) v1 declares `reads:
//! ["orders"]` and runs `mcphost.table.query('SELECT 1 FROM "orders"')` at run time, When `a` runs
//! once and v2 (no declared reads, no `orders` in its source) is then published and never runs,
//! Then `host.lineage.trace` of `table:orders` lists `tool:tool_a` exactly once with evidence
//! `runtime_query` after both steps, and after `host.tool_remove` of `a` no edge in the tenant
//! names `tool:tool_a`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const A_V1: &str = "import mcphost\n\
def main(args):\n\
\x20   mcphost.table.query('SELECT 1 FROM \"orders\"')\n\
\x20   return {\"ran\": True}\n";

const A_V2: &str = "def main(args):\n    return {\"ran\": False}\n";

/// The python kind may still be building its env on the first call.
async fn call_until_ok(client: &McpClient, tool: &str, args: Value) -> Value {
    let mut last = String::new();
    for _ in 0..80 {
        match client.tools_call(tool, args.clone()).await {
            Ok(r) => return extract_structured(&r),
            Err(e) => last = format!("{} {}", e.code, e.message),
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("{tool} never succeeded: {last}");
}

async fn tool_a_entries(client: &McpClient) -> Vec<Value> {
    let trace = extract_structured(
        &client.tools_call("host.lineage.trace", json!({"id": "table:orders"})).await.expect("trace"),
    );
    trace["downstream"].as_array().expect("downstream").iter().filter(|e| e["id"] == json!("tool:tool_a")).cloned().collect()
}

#[tokio::test]
async fn a_runtime_observation_outlives_republish_and_goes_with_tool_remove() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Linrt AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create orders");
    common::publish(&client, "tool_a", "python", json!({"source": A_V1, "reads": ["orders"]})).await;

    let published = tool_a_entries(&client).await;
    assert_eq!(published.len(), 1, "publish registers one edge: {published:?}");
    assert_eq!(published[0]["evidence"], json!("source_scan"), "publish-derived: {published:?}");

    call_until_ok(&client, "host.tool_call", json!({"name": "tool_a", "args": {}})).await;
    let observed = tool_a_entries(&client).await;
    assert_eq!(observed.len(), 1, "{observed:?}");
    assert_eq!(observed[0]["evidence"], json!("runtime_query"), "the run upgrades the evidence: {observed:?}");

    // v2 declares no reads and names no table; it never runs.
    common::publish(&client, "tool_a", "python", json!({"source": A_V2})).await;
    let after_v2 = tool_a_entries(&client).await;
    assert_eq!(after_v2.len(), 1, "the observed pair survives the republish: {after_v2:?}");
    assert_eq!(after_v2[0]["evidence"], json!("runtime_query"), "{after_v2:?}");

    client.tools_call("host.tool_remove", json!({"name": "tool_a"})).await.expect("remove a");
    assert!(tool_a_entries(&client).await.is_empty(), "no downstream entry after remove");
    let edges = server.state.db.lineage_load_edges(tenant.id).await.expect("edges");
    assert!(
        edges.iter().all(|e| e.upstream_id != "tool:tool_a" && e.downstream_id != "tool:tool_a"),
        "no edge in the tenant names tool:tool_a: {:?}",
        edges.iter().map(|e| (&e.upstream_id, &e.downstream_id, &e.evidence)).collect::<Vec<_>>()
    );
    let nodes = server.state.db.lineage_load_nodes(tenant.id).await.expect("nodes");
    assert!(nodes.iter().all(|n| n.node_id != "tool:tool_a"), "the node is gone too");
}
