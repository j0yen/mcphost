//! PRD-mcphost-performance-guarantees-measured
//! AC4 (P0) -- Given three consecutive samples over target for
//! `free/python_cold`, When `/status.json` is read, Then
//! `perf.free.python_cold.state == "over"`; with no rows it is `"no_data"`.

use crate::common::TestServer;
use mcphost::db::PerfSample;
use serde_json::{Value, json};

async fn status(server: &TestServer) -> Value {
    reqwest::get(format!("{}/status.json", server.base_url))
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("json")
}

#[tokio::test]
async fn pgm_ac04_three_over_target_samples_read_over_and_no_rows_read_no_data() {
    let server = TestServer::start().await;
    let before = status(&server).await;
    assert_eq!(before["perf"]["free"]["python_cold"]["state"], json!("no_data"), "{before}");
    assert_eq!(before["perf"]["free"]["python_cold"]["target_p95_ms"], json!(1500));
    assert_eq!(before["perf"]["free"]["python_cold"]["measured_p95_ms"], Value::Null);

    for (i, p95) in [1600, 1700, 2140].into_iter().enumerate() {
        server
            .state
            .db
            .insert_perf_sample(PerfSample {
                ts: 1_000 + i as i64,
                plan: "free".into(),
                path: "python_cold".into(),
                p50_ms: 900,
                p95_ms: p95,
                n: 20,
            })
            .await
            .expect("insert");
    }
    // A fourth, older-than-streak sample under target must not matter; a
    // path with only one over-target row stays ok.
    server
        .state
        .db
        .insert_perf_sample(PerfSample { ts: 5, plan: "free".into(), path: "echo".into(), p50_ms: 10, p95_ms: 999, n: 20 })
        .await
        .expect("insert");

    let after = status(&server).await;
    let row = &after["perf"]["free"]["python_cold"];
    assert_eq!(row["state"], json!("over"), "{after}");
    assert_eq!(row["measured_p95_ms"], json!(2140));
    assert_eq!(after["perf"]["free"]["echo"]["state"], json!("ok"), "{after}");
    assert_eq!(after["perf"]["pro"]["http"]["state"], json!("no_data"), "{after}");
}
