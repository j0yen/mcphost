//! PRD-mcphost-lineage-runtime-edge-writers AC6 (P0) -- Given python tool `alt_reader` (200
//! `mcphost.table.query` calls alternating over `args["tables"]`), tables `orders` and
//! `customers`, and a trigger on it, When a `host.tool_call` run and a `host.trigger_fire` run
//! execute concurrently with `tables: ["orders", "customers"]`, Then exactly two edges exist into
//! `tool:alt_reader`, one per table, both runs return what a solo run returns, and the log holds
//! no lineage-writer warning.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup_and_make_pro};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const ALT_READER: &str = "import mcphost\n\
def main(args):\n\
\x20   tables = args[\"tables\"]\n\
\x20   n = 0\n\
\x20   for i in range(200):\n\
\x20       mcphost.table.query(\"SELECT count(*) FROM \" + tables[i % len(tables)])\n\
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
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{tool} never succeeded: {last}");
}

/// The finished run's `result`, polled until the run leaves `queued`/`running`.
async fn run_result(client: &McpClient, run_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"));
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            assert_eq!(got["status"], json!("done"), "run: {got:?}");
            return got["result"].clone();
        }
        assert!(Instant::now() < deadline, "run {run_id} never finished");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn concurrent_runs_race_to_one_edge_per_table_and_agree_with_a_solo_run() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let log = common::tracecap::capture_text();
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key, tenant_id) = signup_and_make_pro(&server, "Linrt AC6 Tenant", "cus_linrt_ac06").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let _ = ns;

    for table in ["orders", "customers"] {
        client
            .tools_call("host.table.create", json!({"name": table, "columns": {"id": "integer"}}))
            .await
            .expect("create table");
    }
    common::publish(&client, "alt_reader", "python", json!({"source": ALT_READER})).await;
    let args = json!({"tables": ["orders", "customers"]});
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "alt_reader", "schedule": "0 0 1 1 *", "args": args}))
            .await
            .expect("trigger set"),
    );
    let trigger_id = set["id"].as_str().expect("trigger id").to_string();

    // Warm the sandbox env on a dry run, which writes no edge.
    call_until_ok(&client, "host.tool_run", json!({"name": "alt_reader", "args": args, "test": true})).await;
    assert!(
        server.state.db.lineage_load_edges(tenant_id).await.expect("edges").iter().all(|e| e.downstream_id != "tool:alt_reader"),
        "the dry run wrote no edge"
    );

    // Both runs hit the same two new pairs at once.
    let (called, fired) = tokio::join!(
        client.tools_call("host.tool_call", json!({"name": "alt_reader", "args": args})),
        client.tools_call("host.trigger.fire", json!({"id": trigger_id})),
    );
    let called = extract_structured(&called.expect("tool_call"));
    let fired = extract_structured(&fired.expect("trigger fire"));
    let fired_result = run_result(&client, fired["run_id"].as_str().expect("run_id")).await;

    let edges: Vec<_> = server
        .state
        .db
        .lineage_load_edges(tenant_id)
        .await
        .expect("edges")
        .into_iter()
        .filter(|e| e.downstream_id == "tool:alt_reader")
        .collect();
    let mut upstreams: Vec<&str> = edges.iter().map(|e| e.upstream_id.as_str()).collect();
    upstreams.sort();
    assert_eq!(upstreams, vec!["table:customers", "table:orders"], "exactly one edge per table: {edges:?}");
    assert!(edges.iter().all(|e| e.evidence == "runtime_query"), "edges: {edges:?}");

    // A solo run of each returns the same thing.
    let solo_called = call_until_ok(&client, "host.tool_call", json!({"name": "alt_reader", "args": args})).await;
    let solo_fired = extract_structured(&client.tools_call("host.trigger.fire", json!({"id": trigger_id})).await.expect("solo fire"));
    let solo_fired_result = run_result(&client, solo_fired["run_id"].as_str().expect("run_id")).await;
    // `call_id` is a per-call counter; everything else is the tool's own return.
    let without_call_id = |mut v: Value| {
        v.as_object_mut().expect("object result").remove("call_id");
        v
    };
    assert_eq!(
        without_call_id(called.clone()),
        without_call_id(solo_called),
        "the concurrent tool_call returns what a solo one does"
    );
    assert_eq!(called["queries"], json!(200), "{called:?}");
    assert_eq!(fired_result, solo_fired_result, "the concurrent trigger run returns what a solo one does");

    // A trigger run alone (the job executor's own bridge) writes its edges too.
    common::publish(&client, "job_reader", "python", json!({"source": ALT_READER})).await;
    let job_trigger = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "job_reader", "schedule": "0 0 1 1 *", "args": args}))
            .await
            .expect("job trigger set"),
    );
    let job_fire = extract_structured(
        &client.tools_call("host.trigger.fire", json!({"id": job_trigger["id"]})).await.expect("job fire"),
    );
    run_result(&client, job_fire["run_id"].as_str().expect("run_id")).await;
    let job_edges: Vec<_> = server
        .state
        .db
        .lineage_load_edges(tenant_id)
        .await
        .expect("edges")
        .into_iter()
        .filter(|e| e.downstream_id == "tool:job_reader")
        .collect();
    assert_eq!(job_edges.len(), 2, "a job run writes one edge per table: {job_edges:?}");
    assert!(job_edges.iter().all(|e| e.evidence == "runtime_query"), "edges: {job_edges:?}");

    let text = log.text();
    assert!(!text.contains("writer failed"), "no lineage-writer warning in the log: {text}");
}
