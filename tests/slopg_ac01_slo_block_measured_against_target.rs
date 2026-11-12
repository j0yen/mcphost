//! PRD-mcphost-slo-targets-on-status AC1 (P0) — Given a test DB with 120
//! finished `calls` rows in the last 24 h of which 100 are `ok` with known
//! durations, 3 carry a code that `ERROR_CODES` marks `class == HostFault`
//! and 17 are `tool_exception`, When `/status.json` is fetched, Then
//! `slo.tool_call_p95_ms_24h.measured` equals the test's own nearest-rank p95
//! of the 100 durations, `slo.error_rate_24h.measured` is 2.5,
//! `slo.tool_call_p95_ms_24h.met` is `true` and `slo.error_rate_24h.met` is
//! `false` against target 0.5.

use crate::common::{TestServer, signup};
use serde_json::Value;

async fn status(server: &TestServer) -> Value {
    reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body")
}

#[tokio::test]
async fn slopg_ac01_slo_block_measures_p95_and_error_rate_against_targets() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Slo Tenant").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();

    let host_fault = mcphost::error_registry::ERROR_CODES
        .iter()
        .find(|e| e.class == mcphost::error_registry::ErrorClass::HostFault)
        .expect("the registry has a host-fault code")
        .code;

    // 100 ok rows with known, shuffled durations 10, 20, ... 1000 ms.
    let mut durations: Vec<i64> = (1..=100).map(|i| i * 10).collect();
    for (i, d) in durations.iter().enumerate() {
        let at = now - 60 - (i as i64 * 7) % 3_000;
        server.state.db.insert_slo_call_row_for_test(tenant.id, at, *d, true, None, "human").await.expect("ok row");
    }
    for i in 0..3 {
        server
            .state
            .db
            .insert_slo_call_row_for_test(tenant.id, now - 120 - i, 50, false, Some(host_fault.to_string()), "human")
            .await
            .expect("host-fault row");
    }
    for i in 0..17 {
        server
            .state
            .db
            .insert_slo_call_row_for_test(tenant.id, now - 180 - i, 50, false, Some("tool_exception".to_string()), "human")
            .await
            .expect("tool_exception row");
    }

    // The test's own nearest-rank p95: the ceil(0.95 * n)-th smallest.
    durations.sort_unstable();
    let expected_p95 = durations[(0.95_f64 * durations.len() as f64).ceil() as usize - 1];

    let body = status(&server).await;
    let slo = &body["slo"];
    let p95 = &slo["tool_call_p95_ms_24h"];
    assert_eq!(p95["measured"].as_i64(), Some(expected_p95), "p95 block: {p95}");
    assert_eq!(p95["met"], Value::Bool(true), "p95 {expected_p95} ms is under 2000: {p95}");
    assert_eq!(p95["target"].as_f64(), Some(2000.0));
    assert_eq!(p95["source"], "calls");
    assert_eq!(p95["sample_n"].as_i64(), Some(100));

    let rate = &slo["error_rate_24h"];
    assert_eq!(rate["measured"].as_f64(), Some(2.5), "3 host faults of 120 rows: {rate}");
    assert_eq!(rate["met"], Value::Bool(false), "2.5 % is over the 0.5 % target: {rate}");
    assert_eq!(rate["target"].as_f64(), Some(0.5));
    assert_eq!(rate["source"], "calls");
    assert_eq!(rate["sample_n"].as_i64(), Some(120));

    // The block always carries all three keys, even with no rollup rows yet.
    let avail = &slo["availability_30d"];
    assert_eq!(avail["source"], "status_daily");
    assert_eq!(avail["measured"], Value::Null);
    assert_eq!(avail["met"], Value::Null);
}
