//! PRD-mcphost-slo-targets-on-status AC3 (P0) — Given 30 days of
//! `status_daily` rows for 4 components with one component down for a whole
//! day, When `/status.json` is fetched, Then `slo.availability_30d.measured`
//! equals the test's own ok/total over all 120 component-days and `met` is
//! `false` against target 99.5.

use crate::common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};
use serde_json::Value;

const SAMPLES_PER_DAY: i64 = 1_440;

#[tokio::test]
async fn slopg_ac03_availability_is_ok_over_total_of_all_component_days() {
    let server = TestServer::start().await;
    let now = now_unix();
    let components = mcphost::statusfeed::COMPONENTS;
    assert_eq!(components.len(), 4);

    let (mut ok_sum, mut total_sum) = (0_i64, 0_i64);
    for days_ago in 1..=30 {
        let day = rfc3339_from_unix(now - days_ago * 86_400)[..10].to_string();
        for (i, component) in components.iter().enumerate() {
            // `billing` is down for the whole of day 10; every other component-day is clean.
            let ok = if days_ago == 10 && i == 2 { 0 } else { SAMPLES_PER_DAY };
            server
                .state
                .db
                .upsert_status_daily(component.to_string(), day.clone(), ok, SAMPLES_PER_DAY, 5)
                .await
                .expect("status_daily row");
            ok_sum += ok;
            total_sum += SAMPLES_PER_DAY;
        }
    }
    // A fully-down day outside the 30 d window must not count.
    let old_day = rfc3339_from_unix(now - 60 * 86_400)[..10].to_string();
    server.state.db.upsert_status_daily("mcp".to_string(), old_day, 0, SAMPLES_PER_DAY, 5).await.expect("old row");

    let body: Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");
    let a = &body["slo"]["availability_30d"];

    let expected = ok_sum as f64 / total_sum as f64 * 100.0;
    let measured = a["measured"].as_f64().expect("measured is a number");
    assert!((measured - expected).abs() < 1e-9, "measured {measured} != ok/total {expected}: {a}");
    assert_eq!(a["met"], Value::Bool(false), "{expected} % is under the 99.5 target: {a}");
    assert_eq!(a["target"].as_f64(), Some(99.5));
    assert_eq!(a["source"], "status_daily");
    assert_eq!(a["window_secs"].as_i64(), Some(30 * 86_400));
    assert_eq!(a["sample_n"].as_i64(), Some(total_sum));
}

#[tokio::test]
async fn slopg_ac03_clean_month_meets_the_target() {
    let server = TestServer::start().await;
    let now = now_unix();
    for days_ago in 1..=30 {
        let day = rfc3339_from_unix(now - days_ago * 86_400)[..10].to_string();
        for component in mcphost::statusfeed::COMPONENTS {
            server
                .state
                .db
                .upsert_status_daily(component.to_string(), day.clone(), SAMPLES_PER_DAY, SAMPLES_PER_DAY, 5)
                .await
                .expect("status_daily row");
        }
    }
    let body: Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");
    let a = &body["slo"]["availability_30d"];
    assert_eq!(a["measured"].as_f64(), Some(100.0), "{a}");
    assert_eq!(a["met"], Value::Bool(true), "{a}");
}
