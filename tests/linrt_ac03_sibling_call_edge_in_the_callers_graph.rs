//! PRD-mcphost-lineage-runtime-edge-writers AC3 (P0) -- Given B's `caller` (runs
//! `mcphost.call(args["target"])`) and `callee` (dynamic read of `orders`), When `caller` runs with
//! `target: "callee"`, then with A's shared tool, Then B's trace of `tool:callee` lists
//! `tool:caller` (evidence `runtime_call`), B's blast radius of `orders` lists `callee` (depth 1)
//! and `caller` (depth 2), A's tool is in B's graph under the name passed, and A's graph names
//! neither B nor `caller`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const CALLER: &str = "import mcphost\n\
def main(args):\n\
\x20   try:\n\
\x20       return {\"result\": mcphost.call(args[\"target\"], {})}\n\
\x20   except Exception as e:\n\
\x20       return {\"error\": str(e)}\n";

// The table name is computed, so the publish-time scan has nothing to read.
const CALLEE: &str = "import mcphost\n\
def main(args):\n\
\x20   name = \"ord\" + \"ers\"\n\
\x20   mcphost.table.query(\"SELECT count(*) FROM \" + name)\n\
\x20   return {\"ok\": True}\n";

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

async fn trace(client: &McpClient, id: &str) -> Value {
    extract_structured(&client.tools_call("host.lineage.trace", json!({"id": id})).await.expect("trace"))
}

fn entry<'a>(list: &'a Value, id: &str) -> Option<&'a Value> {
    list.as_array().and_then(|l| l.iter().find(|e| e["id"] == json!(id)))
}

#[tokio::test]
async fn a_sibling_call_is_an_edge_in_the_callers_own_graph() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns_a, key_a) = signup(&server.base_url, "Linrt AC3 Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Linrt AC3 Caller B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let tenant_a = server.state.db.find_tenant_by_namespace(ns_a.clone()).await.expect("db").expect("tenant a");
    let tenant_b = server.state.db.find_tenant_by_namespace(ns_b.clone()).await.expect("db").expect("tenant b");

    b.tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create orders");
    common::publish(&b, "callee", "python", json!({"source": CALLEE})).await;
    common::publish(&b, "caller", "python", json!({"source": CALLER})).await;
    common::publish(&a, "shared_t", "echo", json!({"schema": {"type": "object"}})).await;
    a.tools_call("host.tool_share", json!({"name": "shared_t", "visibility": "public"})).await.expect("share");

    let ran = call_until_ok(&b, "host.tool_call", json!({"name": "caller", "args": {"target": "callee"}})).await;
    assert_eq!(ran["result"]["ok"], json!(true), "caller reached callee: {ran:?}");

    let callee_trace = trace(&b, "tool:callee").await;
    let edge = entry(&callee_trace["downstream"], "tool:caller")
        .unwrap_or_else(|| panic!("tool:caller downstream of tool:callee: {callee_trace:?}"));
    assert_eq!(edge["evidence"], json!("runtime_call"), "edge: {edge:?}");

    let blast = extract_structured(
        &b.tools_call("host.lineage.blast_radius", json!({"id": "table:orders", "change_kind": "drop"}))
            .await
            .expect("blast radius"),
    );
    let impacted = &blast["impacted"];
    assert_eq!(entry(impacted, "tool:callee").map(|e| e["depth"].clone()), Some(json!(1)), "blast: {blast:?}");
    assert_eq!(entry(impacted, "tool:caller").map(|e| e["depth"].clone()), Some(json!(2)), "blast: {blast:?}");

    // The same caller, now naming A's shared tool: the call itself is not dispatchable from a
    // composed context, but the name resolves for B, so it is a dependency of `caller`.
    let shared_name = format!("{ns_a}.shared_t");
    call_until_ok(&b, "host.tool_call", json!({"name": "caller", "args": {"target": shared_name}})).await;
    let shared_id = format!("tool:{shared_name}");
    let shared_trace = trace(&b, &shared_id).await;
    let shared_edge = entry(&shared_trace["downstream"], "tool:caller")
        .unwrap_or_else(|| panic!("tool:caller downstream of {shared_id}: {shared_trace:?}"));
    assert_eq!(shared_edge["evidence"], json!("runtime_call"), "edge: {shared_edge:?}");
    let nodes_b = server.state.db.lineage_load_nodes(tenant_b.id).await.expect("nodes b");
    assert!(nodes_b.iter().any(|n| n.node_id == shared_id), "A's tool is in B's graph under the name passed");

    // A's graph names neither B nor `caller`.
    let edges_a = server.state.db.lineage_load_edges(tenant_a.id).await.expect("edges a");
    let nodes_a = server.state.db.lineage_load_nodes(tenant_a.id).await.expect("nodes a");
    let names_b_or_caller = |s: &str| s.contains(&ns_b) || s.contains("caller");
    assert!(
        edges_a.iter().all(|e| !names_b_or_caller(&e.upstream_id) && !names_b_or_caller(&e.downstream_id)),
        "A's edges: {:?}",
        edges_a.iter().map(|e| (&e.upstream_id, &e.downstream_id)).collect::<Vec<_>>()
    );
    assert!(nodes_a.iter().all(|n| !names_b_or_caller(&n.node_id)), "A's nodes name neither B nor caller");
}
