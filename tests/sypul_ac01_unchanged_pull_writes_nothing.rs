//! PRD-mcphost-sync-pull-with-change-detection AC1 -- Given the 5,000-row
//! fixture CSV served with ETag, When a test runs 10 naive full-rewrite pulls
//! and 10 change-detected pulls and prints both wall times and write counts,
//! Then the numbers are printed and the test asserts the change-detected
//! unchanged pull does < 1 table write (a naive pull under 1 s and 1 % of the
//! call budget is the falsifier and parks R2).

use std::time::Instant;

use crate::common::sypul::{Fixture, orders_csv};
use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::sync::pull::RunOptions;
use serde_json::{Value, json};

const ROWS: usize = 5000;
const PULLS: usize = 10;

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn create(client: &McpClient, url: &str, table: &str) -> String {
    let created = call(
        client,
        "host.sync.create",
        json!({"source": {"kind": "csv_url", "ref": url}, "table": table, "primary_key": "id"}),
    )
    .await;
    created["sync_id"].as_str().expect("sync_id").to_string()
}

#[tokio::test]
async fn sypul_ac01_unchanged_pull_of_5000_rows_writes_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SYPUL AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let fixture = Fixture::start(&orders_csv(ROWS), true).await;

    let naive_id = create(&client, &fixture.url("naive.csv"), "naive_orders").await;
    let smart_id = create(&client, &fixture.url("smart.csv"), "smart_orders").await;
    let now = mcphost::state::now_unix();

    let mut naive_writes = 0;
    let started = Instant::now();
    for _ in 0..PULLS {
        let o = mcphost::sync::run_with(&server.state, &naive_id, now, RunOptions { change_detection: false })
            .await
            .expect("naive pull");
        assert_eq!(o.status, "ok", "{o:?}");
        naive_writes += o.writes();
    }
    let naive_ms = started.elapsed().as_millis();

    let hits_before = fixture.hits();
    let mut smart_writes = Vec::new();
    let started = Instant::now();
    for _ in 0..PULLS {
        let o = mcphost::sync::run_with(&server.state, &smart_id, now, RunOptions::default())
            .await
            .expect("change-detected pull");
        assert_eq!(o.status, "ok", "{o:?}");
        smart_writes.push((o.writes(), o.provider_calls));
    }
    let smart_ms = started.elapsed().as_millis();
    println!(
        "sypul_ac01: {PULLS} naive pulls of {ROWS} rows: {naive_ms} ms, {naive_writes} table writes; \
         {PULLS} change-detected pulls: {smart_ms} ms, {} table writes ({} provider calls)",
        smart_writes.iter().map(|(w, _)| w).sum::<i64>(),
        fixture.hits() - hits_before,
    );

    assert_eq!(smart_writes[0].0, ROWS as i64, "the first change-detected pull loads the table: {smart_writes:?}");
    for (i, (writes, calls)) in smart_writes.iter().enumerate().skip(1) {
        assert!(*writes < 1, "unchanged pull {} wrote {writes} rows", i + 1);
        assert_eq!(*calls, 1, "unchanged pull {} spent {calls} provider calls", i + 1);
    }
    assert!(naive_writes >= (PULLS * ROWS) as i64, "a naive pull rewrites every row: {naive_writes}");
    let count = call(&client, "host.table.count", json!({"table": "smart_orders"})).await;
    assert_eq!(count["count"], ROWS, "{count}");
}
