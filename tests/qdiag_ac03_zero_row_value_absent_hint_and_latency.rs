//! PRD-mcphost-query-diagnosis
//! AC3 — Given `SELECT * FROM expenses WHERE category = 'Produce'`
//! returning zero rows, When diagnosed, Then the value `Produce` is
//! `absent` with `produce` among `top_candidates` and the hint names it,
//! and the query's own latency rose by at most 5 ms p95 across 100 runs.
//!
//! The latency clause needs a without-diagnosis baseline to diff against:
//! `diagnose_sync` (src/tables.rs) takes its fast `Ok(_) => None` path for
//! any non-empty result, so a query that matches a row never pays the
//! zero-row value-diagnosis cost (schema load, a second `SELECT DISTINCT`,
//! scoring). That undiagnosed query is the baseline; the zero-row query is
//! the diagnosed case. Same relative-p95-budget shape
//! `hybrid_ac07_p95_latency_within_30ms_of_embeddings_only.rs` uses.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn zero_row_value_filter_is_absent_with_closest_candidate_hinted() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "QDiag AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.table.create",
            json!({"name": "expenses", "columns": {"category": "text", "amount": "real"}}),
        )
        .await
        .expect("create");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "expenses", "rows": [
                {"category": "produce", "amount": 1.0},
                {"category": "meat", "amount": 2.0},
            ]}),
        )
        .await
        .expect("append");

    let sql = "SELECT * FROM expenses WHERE category = 'Produce'";
    let result = extract_structured(
        &client.tools_call("host.table.query", json!({"sql": sql})).await.expect("query"),
    );
    assert_eq!(result["rows"].as_array().expect("rows").len(), 0, "result: {result}");

    let log = extract_structured(
        &client.tools_call("host.table.query_log", json!({"limit": 1})).await.expect("query_log"),
    );
    let entry = &log["rows"][0];
    let identifier = &entry["diagnosis"]["identifiers"][0];
    assert_eq!(identifier["term"], "Produce", "entry: {entry}");
    assert_eq!(identifier["kind"], "value", "entry: {entry}");
    assert_eq!(identifier["status"], "absent", "entry: {entry}");
    let candidate_names: Vec<&str> = identifier["top_candidates"]
        .as_array()
        .expect("top_candidates")
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(candidate_names.contains(&"produce"), "candidates: {candidate_names:?}");

    let hint = entry["hint"].as_str().expect("hint");
    assert!(hint.contains("Produce"), "{hint}");
    assert!(hint.contains("produce"), "{hint}");

    // Baseline: the same table, a WHERE-equality that matches a row, so
    // `diagnose_sync`'s `Ok(_) => None` fast path runs -- no schema load, no
    // second `SELECT DISTINCT`, no scoring. 100 runs each, matching the
    // AC's "across 100 runs" wording.
    let baseline_sql = "SELECT * FROM expenses WHERE category = 'meat'";
    const N: usize = 100;

    let mut baseline_durations = Vec::with_capacity(N);
    for _ in 0..N {
        let start = Instant::now();
        let result = extract_structured(
            &client.tools_call("host.table.query", json!({"sql": baseline_sql})).await.expect("baseline query rep"),
        );
        assert_eq!(result["rows"].as_array().expect("rows").len(), 1, "result: {result}");
        baseline_durations.push(start.elapsed());
    }

    let mut diagnosed_durations = Vec::with_capacity(N);
    for _ in 0..N {
        let start = Instant::now();
        client.tools_call("host.table.query", json!({"sql": sql})).await.expect("diagnosed query rep");
        diagnosed_durations.push(start.elapsed());
    }

    fn p95(mut durations: Vec<Duration>) -> Duration {
        durations.sort();
        durations[(durations.len() as f64 * 0.95).ceil() as usize - 1]
    }

    let baseline_p95 = p95(baseline_durations);
    let diagnosed_p95 = p95(diagnosed_durations);
    let budget = Duration::from_millis(5);
    assert!(
        diagnosed_p95 <= baseline_p95 + budget,
        "diagnosed p95 {diagnosed_p95:?} must not exceed undiagnosed baseline p95 {baseline_p95:?} \
         by more than {budget:?}"
    );
}
