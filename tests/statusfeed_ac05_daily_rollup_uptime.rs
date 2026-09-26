//! PRD-mcphost-status-feed AC5 (P0): given 1,440 samples for one day with 1,425
//! ok, when the daily rollup runs, then `status_daily` shows
//! `ok_samples: 1425` and `/status.json` reports `uptime_30d` computed
//! from the rollup.

use crate::common;

use common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};

const TOTAL_SAMPLES: i64 = 1_440;
const OK_SAMPLES: i64 = 1_425;

#[tokio::test]
async fn daily_rollup_computes_uptime_30d() {
    let server = TestServer::start().await;

    let day_start = now_unix().div_euclid(86_400) * 86_400;
    let day = rfc3339_from_unix(day_start)[..10].to_string();

    for i in 0..TOTAL_SAMPLES {
        let ok = i < OK_SAMPLES;
        server
            .state
            .db
            .insert_status_sample("mcp".to_string(), day_start + i * 60, ok, 10, "self".to_string())
            .await
            .expect("insert_status_sample");
    }

    mcphost::statusfeed::rollup_day(&server.state, "mcp", &day)
        .await
        .expect("rollup_day");

    let rows = server
        .state
        .db
        .status_daily_recent("mcp".to_string(), 1)
        .await
        .expect("status_daily_recent");
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0].day, day);
    assert_eq!(rows[0].ok_samples, OK_SAMPLES);
    assert_eq!(rows[0].total_samples, TOTAL_SAMPLES);

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    let mcp = body["components"]
        .as_array()
        .expect("components array")
        .iter()
        .find(|c| c["name"] == "mcp")
        .expect("mcp component present");
    let uptime_30d = mcp["uptime_30d"].as_f64().expect("uptime_30d present");
    let expected = OK_SAMPLES as f64 / TOTAL_SAMPLES as f64;
    assert!(
        (uptime_30d - expected).abs() < 1e-9,
        "uptime_30d {uptime_30d} != expected {expected}"
    );
}
