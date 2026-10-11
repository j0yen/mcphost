//! PRD-mcphost-data-quota-in-every-response
//! AC6 -- Given a free-plan tenant at 5,000 state rows, 5 tables of 2,000
//! rows, 50 docs, When `Db::data_usage` runs 100 times, Then p95 <= 5 ms;
//! Given two concurrent appends, Then both responses carry `used` values
//! that differ by the other's rows or equal the final total.
//!
//! The latency bound is skipped under `MCPHOST_PERF_SKIP=1` (a loaded host;
//! the gate box is the truth for it); the figures and the concurrency
//! clause are always asserted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::time::Instant;

fn n_rows(from: i64, count: i64) -> Value {
    Value::Array((from..from + count).map(|n| json!({"n": n})).collect())
}

#[tokio::test]
async fn data_usage_p95_under_5ms_and_concurrent_appends_report_consistent_used() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.state.table_create", json!({"name": "s", "schema": {"n": "integer"}}))
        .await
        .expect("state table");
    for chunk in 0..25 {
        client
            .tools_call("host.state.insert", json!({"table": "s", "rows": n_rows(chunk * 200, 200)}))
            .await
            .unwrap_or_else(|e| panic!("state insert {chunk}: {e:?}"));
    }
    for t in 0..5 {
        let name = format!("t{t}");
        client
            .tools_call("host.table.create", json!({"name": name, "columns": {"n": "integer"}}))
            .await
            .expect("table create");
        client
            .tools_call("host.table.append", json!({"table": name, "rows": n_rows(0, 2000)}))
            .await
            .expect("table append");
    }
    for d in 0..50 {
        client
            .tools_call("host.docs.put", json!({"name": format!("d{d}.md"), "content": format!("doc {d}")}))
            .await
            .expect("docs put");
    }

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    let mut ms = Vec::new();
    let mut last = Default::default();
    for _ in 0..100 {
        let t0 = Instant::now();
        last = server.state.db.data_usage(tenant.id).await.expect("data_usage");
        ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    assert_eq!(last.used["state_rows_max"], 5000);
    assert_eq!(last.used["table_tables_max"], 5);
    assert_eq!(last.used["table_rows_max"], 2000);
    assert_eq!(last.used["docs_max"], 50);
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = ms[94];
    println!("data_usage p95 = {p95:.3} ms");
    // 5 ms is the optimized-build budget (measured on the gate box); an
    // unoptimized test build is held to a 10x ceiling so a regression to
    // per-row work still fails here.
    let budget_ms = if cfg!(debug_assertions) { 50.0 } else { 5.0 };
    if std::env::var("MCPHOST_PERF_SKIP").ok().as_deref() != Some("1") {
        assert!(p95 <= budget_ms, "data_usage p95 {p95:.3} ms > {budget_ms} ms");
    }

    // Two concurrent appends of 10 rows each into the 2,000-row table t0.
    let (a, b) = tokio::join!(
        client.tools_call("host.table.append", json!({"table": "t0", "rows": n_rows(5000, 10)})),
        client.tools_call("host.table.append", json!({"table": "t0", "rows": n_rows(6000, 10)})),
    );
    let used: Vec<i64> = [a, b]
        .into_iter()
        .map(|r| {
            let body = extract_structured(&r.expect("append"));
            body["data"]["quota"]["table_rows_max"]["used"].as_i64().expect("used")
        })
        .collect();
    let final_total = 2020;
    for u in &used {
        assert!(*u == 2010 || *u == final_total, "used {u} must be base+own rows or the final total: {used:?}");
    }
    assert!(used.contains(&final_total), "the later response sees the final total: {used:?}");
    assert_eq!(server.state.db.data_usage(tenant.id).await.unwrap().used["table_rows_max"], final_total);
}
