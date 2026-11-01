//! PRD-mcphost-lineage-runtime-edge-writers AC4 (P0) -- Given tenant A shares tool `src` (source
//! reads `orders` literally) with fork scope, When tenant B forks it as `src_fork`, Then B's
//! `host.lineage.trace` of `table:orders` lists `tool:src_fork` with evidence `source_scan`, A's
//! graph is unchanged, and a fork B is not permitted to make writes no node and no edge for B.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use serde_json::{Value, json};

const SRC: &str = "import mcphost\n\
def main(args):\n\
\x20   return mcphost.table.query(\"SELECT * FROM orders\")\n";

async fn graph_of(server: &TestServer, tenant_id: i64) -> (Vec<(String, String, String)>, Vec<String>) {
    let mut edges: Vec<_> = server
        .state
        .db
        .lineage_load_edges(tenant_id)
        .await
        .expect("edges")
        .into_iter()
        .map(|e| (e.upstream_id, e.downstream_id, e.evidence))
        .collect();
    edges.sort();
    let mut nodes: Vec<_> =
        server.state.db.lineage_load_nodes(tenant_id).await.expect("nodes").into_iter().map(|n| n.node_id).collect();
    nodes.sort();
    (edges, nodes)
}

#[tokio::test]
async fn a_fork_has_the_edges_a_publish_would_give_it() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns_a, key_a) = signup(&server.base_url, "Linrt AC4 Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Linrt AC4 Fork B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let tenant_a = server.state.db.find_tenant_by_namespace(ns_a.clone()).await.expect("db").expect("tenant a");
    let tenant_b = server.state.db.find_tenant_by_namespace(ns_b.clone()).await.expect("db").expect("tenant b");

    // `src` can be forked; `callonly` cannot.
    for name in ["src", "callonly"] {
        a.tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": {"source": SRC}}))
            .await
            .expect("publish");
    }
    a.tools_call("host.group.create", json!({"name": "crew"})).await.expect("group create");
    a.tools_call("host.group.add", json!({"name": "crew", "namespace": ns_b})).await.expect("group add");
    for (name, scopes) in [("src", json!(["fork"])), ("callonly", json!(["call"]))] {
        a.tools_call("host.tool_share", json!({"name": name, "visibility": "group", "group": "crew", "scopes": scopes}))
            .await
            .expect("share");
    }

    let a_before = graph_of(&server, tenant_a.id).await;
    assert!(
        a_before.0.iter().any(|(up, down, _)| up == "table:orders" && down == "tool:src"),
        "A's own publish registered its edge: {a_before:?}"
    );
    assert!(graph_of(&server, tenant_b.id).await.0.is_empty(), "B starts with an empty graph");

    b.tools_call("host.tool_fork", json!({"owner": ns_a, "tool": "src", "as": "src_fork"}))
        .await
        .unwrap_or_else(|e| panic!("fork: {} {}", e.code, e.message));
    let traced: Value = extract_structured(&b.tools_call("host.lineage.trace", json!({"id": "table:orders"})).await.expect("trace"));
    let fork_edge = traced["downstream"]
        .as_array()
        .expect("downstream")
        .iter()
        .find(|e| e["id"] == json!("tool:src_fork"))
        .unwrap_or_else(|| panic!("tool:src_fork in B's trace: {traced:?}"));
    assert_eq!(fork_edge["evidence"], json!("source_scan"), "edge: {fork_edge:?}");
    assert_eq!(graph_of(&server, tenant_a.id).await, a_before, "A's graph is unchanged by B's fork");

    // A fork B is not permitted to make writes nothing.
    let b_after_fork = graph_of(&server, tenant_b.id).await;
    let err = b
        .tools_call("host.tool_fork", json!({"owner": ns_a, "tool": "callonly", "as": "nope"}))
        .await
        .expect_err("a call-only share refuses fork");
    assert_eq!(err.error_code.as_deref(), Some("scope_missing"), "{}", err.message);
    assert_eq!(graph_of(&server, tenant_b.id).await, b_after_fork, "a refused fork writes no node and no edge");
    assert_eq!(graph_of(&server, tenant_a.id).await, a_before, "and A's graph is still unchanged");
}
