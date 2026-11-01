//! PRD-mcphost-lineage-runtime-edge-writers AC1 (P0) -- Given python tool `dyn_reader` whose source
//! runs `mcphost.table.query("SELECT count(*) FROM " + args["table"])` 1,000 times per call, When
//! `host.tool_call` then `host.tool_run` each run it once with `table: "orders"`, Then
//! `host.lineage.trace` of `table:orders` lists `tool:dyn_reader` once with evidence
//! `runtime_query`, the graph cache was dropped once, and wall time is under 2x the same run with
//! `tool_name` unset.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const DYN_READER: &str = "import mcphost\n\
def main(args):\n\
\x20   n = 0\n\
\x20   for _ in range(1000):\n\
\x20       mcphost.table.query(\"SELECT count(*) FROM \" + args[\"table\"])\n\
\x20       n += 1\n\
\x20   return {\"queries\": n}\n";

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

fn orders_edges(trace: &Value) -> Vec<Value> {
    trace["downstream"]
        .as_array()
        .expect("downstream array")
        .iter()
        .filter(|e| e["id"] == json!("tool:dyn_reader"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn dynamic_reader_writes_one_runtime_edge_and_drops_the_cache_once() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Linrt AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"id": "integer"}}))
        .await
        .expect("create orders");
    // A source that builds its SQL from its arguments has no literal for the publish-time scan.
    common::publish(&client, "dyn_reader", "python", json!({"source": DYN_READER})).await;
    let before = client
        .tools_call("host.lineage.trace", json!({"id": "table:orders"}))
        .await
        .map(|r| extract_structured(&r))
        .expect("trace before any run");
    assert!(orders_edges(&before).is_empty(), "publish writes no edge for a dynamic reader: {before:?}");

    // Warm the sandbox env on a dry run (writes nothing), then count drops across the two real runs.
    call_until_ok(&client, "host.tool_run", json!({"name": "dyn_reader", "args": {"table": "orders"}, "test": true})).await;
    let drops_before = server.state.lineage_cache.invalidations();

    let called = call_until_ok(&client, "host.tool_call", json!({"name": "dyn_reader", "args": {"table": "orders"}})).await;
    let ran = call_until_ok(&client, "host.tool_run", json!({"name": "dyn_reader", "args": {"table": "orders"}})).await;
    assert_eq!(called["queries"], json!(1000), "tool_call result: {called:?}");
    assert_eq!(ran["payload"]["queries"], json!(1000), "tool_run result: {ran:?}");

    let trace = extract_structured(
        &client.tools_call("host.lineage.trace", json!({"id": "table:orders"})).await.expect("trace after runs"),
    );
    let edges = orders_edges(&trace);
    assert_eq!(edges.len(), 1, "tool:dyn_reader exactly once: {trace:?}");
    assert_eq!(edges[0]["evidence"], json!("runtime_query"), "edge: {:?}", edges[0]);
    assert_eq!(
        server.state.lineage_cache.invalidations() - drops_before,
        1,
        "1,000 queries in each of two runs drop the graph cache once"
    );

    // Wall time: a real run against the same source with the bridge's tool_name unset
    // (`host.spec_test` builds a real, non-dry bridge that names no tool).
    if common::perf_skipped() {
        eprintln!("perf skipped (load)");
        return;
    }
    let run_args = json!({"name": "dyn_reader", "args": {"table": "orders"}});
    let spec_args = json!({"kind": "python", "spec": {"source": DYN_READER}, "invocations": [{"table": "orders"}]});
    let unset = crate::perf_median!({
        client.tools_call("host.spec_test", spec_args.clone()).await.expect("spec_test run");
    });
    let named = crate::perf_median!({
        client.tools_call("host.tool_run", run_args.clone()).await.expect("tool_run run");
    });
    eprintln!("linrt_ac01: tool_name unset median={unset:?} named median={named:?}");
    assert!(named < unset * 2, "named run {named:?} must stay under 2x the unset run {unset:?}");
}
