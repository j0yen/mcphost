//! PRD-mcphost-export-data-class-completeness-v2
//! AC4 -- Given an export running at 100/400 entries, When `host.export` is
//! called again, Then `export_in_progress {run_id, entries_done: 100,
//! entries_total: 400}` and no second run row exists; When `host.runs.get`
//! is read, Then the same counters.

use crate::common;
use crate::expc;

use common::TestServer;
use mcphost::export::EXPORT_TOOL_NAME;
use serde_json::{Value, json};

fn count_export_runs(server: &TestServer, tenant_id: i64) -> i64 {
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE tenant_id = ?1 AND tool_name = ?2",
        rusqlite::params![tenant_id, EXPORT_TOOL_NAME],
        |r| r.get(0),
    )
    .expect("count runs")
}

#[tokio::test]
async fn expc_ac04() {
    let server = TestServer::start().await;
    let tenant = expc::new_tenant(&server, "Expc AC4 Tenant").await;
    let row = server.state.db.find_tenant_by_namespace(tenant.ns.clone()).await.expect("query").expect("tenant");

    // An export that is mid-run at 100 of 400 entries: the run row `host.export`
    // itself inserts, and the progress key it writes while it works.
    let run_id = "01EXPCAC04RUNNINGEXPORT00".to_string();
    server
        .state
        .db
        .start_export_run(row.id, run_id.clone(), EXPORT_TOOL_NAME.to_string(), 600, "{}".to_string())
        .await
        .expect("start run");
    server
        .state
        .db
        .state_kv_set(
            row.id,
            format!("runs/{run_id}/progress"),
            String::new(),
            json!({"entries_done": 100, "entries_total": 400}).to_string(),
        )
        .await
        .expect("write progress");
    assert_eq!(count_export_runs(&server, row.id), 1);

    // A second call is a typed refusal naming the running export and its counters.
    let err = tenant
        .client
        .tools_call("host.export", json!({}))
        .await
        .expect_err("a second host.export while one runs is refused");
    assert_eq!(err.error_code.as_deref(), Some("export_in_progress"), "{err:?}");
    assert_eq!(err.data["run_id"], json!(run_id), "{:?}", err.data);
    assert_eq!(err.data["entries_done"], json!(100), "{:?}", err.data);
    assert_eq!(err.data["entries_total"], json!(400), "{:?}", err.data);
    assert!(err.data["started_unix"].as_i64().is_some_and(|t| t > 0), "{:?}", err.data);
    assert_eq!(count_export_runs(&server, row.id), 1, "no second run row");

    // `host.runs.get` reports the same counters.
    let got: Value = tenant.call("host.runs.get", json!({"run_id": run_id})).await;
    assert_eq!(got["status"], json!("running"), "{got:?}");
    assert_eq!(got["entries_done"], json!(100), "{got:?}");
    assert_eq!(got["entries_total"], json!(400), "{got:?}");

    // Before the collector has counted, the total is null and the done count 0.
    let fresh = expc::new_tenant(&server, "Expc AC4 Fresh").await;
    let fresh_row = server.state.db.find_tenant_by_namespace(fresh.ns.clone()).await.expect("query").expect("tenant");
    server
        .state
        .db
        .start_export_run(fresh_row.id, "01EXPCAC04UNCOUNTEDEXPORT0".to_string(), EXPORT_TOOL_NAME.to_string(), 600, "{}".to_string())
        .await
        .expect("start run");
    let err = fresh.client.tools_call("host.export", json!({})).await.expect_err("refused");
    assert_eq!(err.data["entries_done"], json!(0), "{:?}", err.data);
    assert_eq!(err.data["entries_total"], Value::Null, "{:?}", err.data);
}
