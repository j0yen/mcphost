//! PRD-mcphost-status-feed AC10 (P1): given
//! `GET /status.json?component=mcp&days=7`, when called, then seven daily
//! rows with `ok_samples`, `total_samples`, `p95_latency_ms` return.

use crate::common;

use common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};

#[tokio::test]
async fn component_days_query_returns_seven_daily_rows() {
    let server = TestServer::start().await;

    for day_offset in 0..7i64 {
        let ts = now_unix() - day_offset * 86_400;
        let day = rfc3339_from_unix(ts)[..10].to_string();
        server
            .state
            .db
            .upsert_status_daily("mcp".to_string(), day, 1400 + day_offset, 1440, 20 + day_offset)
            .await
            .expect("upsert_status_daily");
    }

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json?component=mcp&days=7", server.base_url))
        .send()
        .await
        .expect("GET /status.json?component=mcp&days=7");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("parse response");
    assert_eq!(body["component"], "mcp", "body: {body}");

    let days = body["days"].as_array().expect("days array");
    assert_eq!(days.len(), 7, "days: {days:?}");
    for row in days {
        assert!(row["day"].is_string(), "row: {row}");
        assert!(row["ok_samples"].is_i64(), "row: {row}");
        assert!(row["total_samples"].is_i64(), "row: {row}");
        assert!(row["p95_latency_ms"].is_i64(), "row: {row}");
        assert_eq!(row["total_samples"], 1440);
    }
}
