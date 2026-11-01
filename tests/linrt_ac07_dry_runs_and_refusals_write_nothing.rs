//! PRD-mcphost-lineage-runtime-edge-writers AC7 (P0) -- Given a python tool run through
//! `host.tool_test` that queries `orders` and calls a sibling, a spec run through
//! `host.spec_test`, a tool whose statement is `SELECT 1`, and a tool whose statement the table
//! layer rejects, When each runs, Then no edge is written for any of them, each response or error
//! equals the same call's with `tool_name` unset, and `host.lineage.trace` of `table:orders` is
//! unchanged.
//!
//! `host.spec_test` is the "`tool_name` unset" baseline: it runs the same source through a real
//! table bridge that names no tool.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const PROBE: &str = "import mcphost\n\
def main(args):\n\
\x20   mcphost.table.query(\"SELECT count(*) AS c FROM orders\")\n\
\x20   try:\n\
\x20       s = mcphost.call(\"sibling\", {})\n\
\x20   except Exception as e:\n\
\x20       s = \"err:\" + str(e)\n\
\x20   return {\"sibling\": s}\n";

const SELECT_ONE: &str = "import mcphost\n\
def main(args):\n\
\x20   mcphost.table.query(\"SELECT 1\")\n\
\x20   return {\"one\": 1}\n";

const REJECTED: &str = "import mcphost\n\
def main(args):\n\
\x20   try:\n\
\x20       mcphost.table.query(\"DROP TABLE orders\")\n\
\x20   except Exception as e:\n\
\x20       return {\"err\": str(e)}\n\
\x20   return {}\n";

const SIBLING: &str = "def main(args):\n    return {\"s\": 1}\n";

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

/// What `host.spec_test` (no tool named) returns for `source`: the one invocation's `output.result`.
async fn unnamed_result(client: &McpClient, source: &str) -> Value {
    let out = call_until_ok(
        client,
        "host.spec_test",
        json!({"kind": "python", "spec": {"source": source}, "invocations": [{}]}),
    )
    .await;
    out["invocations"][0]["output"]["result"].clone()
}

/// A real run's return, minus the per-call counter.
fn without_call_id(mut v: Value) -> Value {
    v.as_object_mut().expect("object result").remove("call_id");
    v
}

#[tokio::test]
async fn dry_runs_and_refused_statements_leave_the_graph_untouched() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Linrt AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create orders");
    for (name, source) in [("sibling", SIBLING), ("probe", PROBE), ("select_one", SELECT_ONE), ("rejected", REJECTED)] {
        common::publish(&client, name, "python", json!({"source": source})).await;
    }

    // What publish alone registered: probe's literal `FROM orders` (source_scan), nothing else.
    let trace_args = json!({"id": "table:orders"});
    let trace_before = extract_structured(&client.tools_call("host.lineage.trace", trace_args.clone()).await.expect("trace"));
    let edges_before = server.state.db.lineage_load_edges(tenant.id).await.expect("edges");
    let nodes_before = server.state.db.lineage_load_nodes(tenant.id).await.expect("nodes");
    assert_eq!(edges_before.len(), 1, "only probe's publish-time edge exists: {:?}", edges_before.len());
    assert_eq!(edges_before[0].evidence, "source_scan");

    // host.tool_test: queries orders and calls a sibling; its result is the unnamed run's.
    let tested = call_until_ok(&client, "host.tool_test", json!({"name": "probe", "args": {}})).await;
    assert_eq!(tested["result"], unnamed_result(&client, PROBE).await, "tool_test vs spec_test: {tested:?}");

    // Real runs whose statement references no table / is refused by the table layer.
    for (name, source) in [("select_one", SELECT_ONE), ("rejected", REJECTED)] {
        let named = call_until_ok(&client, "host.tool_call", json!({"name": name, "args": {}})).await;
        assert_eq!(without_call_id(named), unnamed_result(&client, source).await, "{name}: named run vs unnamed run");
    }

    let trace_after = extract_structured(&client.tools_call("host.lineage.trace", trace_args).await.expect("trace"));
    assert_eq!(trace_after, trace_before, "trace of table:orders is unchanged");
    let edges_after = server.state.db.lineage_load_edges(tenant.id).await.expect("edges");
    let nodes_after = server.state.db.lineage_load_nodes(tenant.id).await.expect("nodes");
    let edge_rows = |es: &[mcphost::db::LineageEdgeRow]| {
        let mut rows: Vec<_> = es.iter().map(|e| (e.upstream_id.clone(), e.downstream_id.clone(), e.evidence.clone())).collect();
        rows.sort();
        rows
    };
    assert_eq!(edge_rows(&edges_after), edge_rows(&edges_before), "no edge was written or upgraded");
    assert_eq!(nodes_after.len(), nodes_before.len(), "no node was written");
}
