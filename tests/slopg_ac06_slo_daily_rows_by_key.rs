//! PRD-mcphost-slo-targets-on-status AC6 (P1) — Given
//! `?slo=error_rate_24h&days=7`, When fetched, Then 7 or fewer `slo_daily`
//! rows are returned, newest last, each with `day`, `measured`, `sample_n`.
//! Also covers R4: the daily rollup writes the `slo_daily` rows.

use crate::common::{TestServer, signup};
use mcphost::state::{now_unix, rfc3339_from_unix};
use serde_json::Value;

async fn get(server: &TestServer, query: &str) -> (u16, Value) {
    let r = reqwest::Client::new()
        .get(format!("{}/status.json?{query}", server.base_url))
        .send()
        .await
        .expect("GET /status.json");
    (r.status().as_u16(), r.json().await.expect("parse body"))
}

#[tokio::test]
async fn slopg_ac06_days_7_returns_at_most_7_rows_newest_last() {
    let server = TestServer::start().await;
    let now = now_unix();
    // Ten days of rows, written out of order so the sort is the route's, not the insert's.
    for days_ago in [3, 9, 0, 6, 1, 8, 5, 2, 7, 4] {
        let day = rfc3339_from_unix(now - days_ago * 86_400)[..10].to_string();
        server.state.db.upsert_slo_daily(day, "error_rate_24h".into(), Some(days_ago as f64 / 10.0), 100 + days_ago).await.expect("slo_daily");
    }
    // A different key's rows must not leak into the answer.
    let other = rfc3339_from_unix(now)[..10].to_string();
    server.state.db.upsert_slo_daily(other, "availability_30d".into(), Some(99.9), 5).await.expect("other key");

    let (status, body) = get(&server, "slo=error_rate_24h&days=7").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["slo"], "error_rate_24h");
    let rows = body["days"].as_array().expect("days array");
    assert_eq!(rows.len(), 7, "the 7 newest of 10 days: {body}");
    let days: Vec<&str> = rows.iter().map(|r| r["day"].as_str().expect("day")).collect();
    let mut sorted = days.clone();
    sorted.sort_unstable();
    assert_eq!(days, sorted, "newest last: {days:?}");
    assert_eq!(days.last().copied(), Some(&rfc3339_from_unix(now)[..10]), "today is the newest row");
    for r in rows {
        let obj = r.as_object().expect("row object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["day", "measured", "sample_n"], "{r}");
        assert!(r["measured"].is_number() && r["sample_n"].is_i64(), "{r}");
    }

    // Fewer rows than asked for: all of them, still newest last.
    let (_, few) = get(&server, "slo=availability_30d&days=7").await;
    assert_eq!(few["days"].as_array().expect("days").len(), 1, "{few}");
}

#[tokio::test]
async fn slopg_ac06_unknown_slo_key_is_a_400_naming_the_keys() {
    let server = TestServer::start().await;
    let (status, body) = get(&server, "slo=made_up&days=7").await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error_code"], "args_invalid");
    let keys: Vec<&str> = body["keys"].as_array().expect("keys").iter().map(|k| k.as_str().expect("key")).collect();
    let want: Vec<&str> = mcphost::statusfeed::SLO_TARGETS.iter().map(|t| t.key).collect();
    assert_eq!(keys, want);
}

#[tokio::test]
async fn slopg_ac06_the_daily_tick_writes_slo_daily_from_calls_and_status_daily() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Slo Daily").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = now_unix();
    let yesterday_start = (now - 86_400).div_euclid(86_400) * 86_400;
    let yesterday = rfc3339_from_unix(yesterday_start)[..10].to_string();
    let host_fault = mcphost::statusfeed::host_fault_codes()[0];

    // Yesterday: 22 ok rows (durations 10..=220) and 2 host faults of 24.
    for i in 1..=22_i64 {
        server.state.db.insert_slo_call_row_for_test(tenant.id, yesterday_start + 600 + i, i * 10, true, None, "human").await.expect("ok");
    }
    for i in 0..2_i64 {
        server.state.db.insert_slo_call_row_for_test(tenant.id, yesterday_start + 900 + i, 5, false, Some(host_fault.to_string()), "human").await.expect("fault");
    }
    for component in mcphost::statusfeed::COMPONENTS {
        server.state.db.upsert_status_daily(component.to_string(), yesterday.clone(), 1_430, 1_440, 5).await.expect("status_daily");
    }

    mcphost::statusfeed::daily_tick_once(&server.state).await.expect("daily tick");

    let (_, err) = get(&server, "slo=error_rate_24h&days=7").await;
    let row = err["days"].as_array().expect("days").iter().find(|r| r["day"] == yesterday.as_str()).expect("yesterday row").clone();
    assert_eq!(row["sample_n"].as_i64(), Some(24), "{row}");
    assert!((row["measured"].as_f64().expect("measured") - 2.0 / 24.0 * 100.0).abs() < 1e-9, "{row}");

    let (_, p95) = get(&server, "slo=tool_call_p95_ms_24h&days=7").await;
    let row = p95["days"].as_array().expect("days").iter().find(|r| r["day"] == yesterday.as_str()).expect("yesterday row").clone();
    // Nearest rank over the 22 ok durations: ceil(0.95 * 22) = 21st smallest = 210.
    assert_eq!(row["measured"].as_f64(), Some(210.0), "{row}");
    assert_eq!(row["sample_n"].as_i64(), Some(22));

    let (_, avail) = get(&server, "slo=availability_30d&days=7").await;
    let row = avail["days"].as_array().expect("days").iter().find(|r| r["day"] == yesterday.as_str()).expect("yesterday row").clone();
    assert!((row["measured"].as_f64().expect("measured") - 1_430.0 / 1_440.0 * 100.0).abs() < 1e-9, "{row}");
    assert_eq!(row["sample_n"].as_i64(), Some(5_760));

    // Re-running recomputes rather than duplicating.
    mcphost::statusfeed::daily_tick_once(&server.state).await.expect("second tick");
    let (_, again) = get(&server, "slo=error_rate_24h&days=30").await;
    let n = again["days"].as_array().expect("days").iter().filter(|r| r["day"] == yesterday.as_str()).count();
    assert_eq!(n, 1);
}
