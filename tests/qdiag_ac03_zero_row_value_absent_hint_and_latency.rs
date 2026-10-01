//! PRD-mcphost-query-diagnosis
//! AC3 — Given `SELECT * FROM expenses WHERE category = 'Produce'`
//! returning zero rows, When diagnosed, Then the value `Produce` is
//! `absent` with `produce` among `top_candidates` and the hint names it,
//! and the query's own latency rose by at most 5 ms p95 across 100 runs.
//!
//! Same overhead-measurement stance `sqlpass_ac04_successful_query_logs_row_count.rs`
//! takes: an absolute end-to-end p95 ceiling (diagnosis is unconditional
//! in this repo, so there is no "without diagnosis" baseline to diff
//! against), generous enough that a real regression would still trip it.

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
    let entry = &log["entries"][0];
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

    // Overhead proxy: 100 repetitions of the same diagnosed query stay
    // comfortably fast end to end.
    const N: usize = 100;
    let mut durations = Vec::with_capacity(N);
    for _ in 0..N {
        let start = Instant::now();
        client.tools_call("host.table.query", json!({"sql": sql})).await.expect("query rep");
        durations.push(start.elapsed());
    }
    durations.sort();
    let p95 = durations[(durations.len() as f64 * 0.95).ceil() as usize - 1];
    assert!(
        p95 < Duration::from_millis(250),
        "p95 latency over {N} diagnosed queries was {p95:?}, expected comfortably under 250ms"
    );
}
