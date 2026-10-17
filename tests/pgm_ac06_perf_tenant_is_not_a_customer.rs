//! PRD-mcphost-performance-guarantees-measured
//! AC6 (P0) -- Given `host.usage` for the operator, When the `perf-free`
//! tenant has made 480 calls today, Then they do not appear in any
//! customer-facing count and `signup_events` holds no row for it.

use crate::common::{self, McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn pgm_ac06_480_perf_calls_stay_out_of_usage_healthz_and_signup_events() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Pgm AC6 operator").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let signups_before = server.state.db.table_row_count_for_test("signup_events".into()).await.expect("count");

    let before = common::extract_structured(&client.tools_call("host.usage", json!({})).await.expect("usage"));
    let health_before: Value = reqwest::get(format!("{}/healthz", server.base_url)).await.expect("healthz").json().await.expect("json");

    let (perf_id, perf_ns) = mcphost::perf::ensure_tenant(&server.state, "free").await.expect("perf tenant");
    assert_eq!(perf_ns, "perf-free");
    // Worst case: every row stamped `external`, as a real call would be.
    for _ in 0..480 {
        server
            .state
            .db
            .record_call(perf_id, "perf.echo".into(), 1, true, None, None, None, "ok", "external".into(), None)
            .await
            .expect("record call");
    }
    assert_eq!(server.state.db.calls_row_count_for_test(perf_id).await.expect("n"), 480);

    let after = common::extract_structured(&client.tools_call("host.usage", json!({})).await.expect("usage"));
    assert_eq!(after["calls"], before["calls"], "operator host.usage calls moved: {after}");
    let health_after: Value = reqwest::get(format!("{}/healthz", server.base_url)).await.expect("healthz").json().await.expect("json");
    assert_eq!(health_after["calls"], health_before["calls"], "healthz calls moved: {health_after}");
    assert_eq!(health_after["tenants"], health_before["tenants"], "healthz tenants moved: {health_after}");
    assert_eq!(health_after["signups"], health_before["signups"], "{health_after}");
    let signups_after = server.state.db.table_row_count_for_test("signup_events".into()).await.expect("count");
    assert_eq!(signups_after, signups_before, "perf tenant creation wrote a signup_events row");
}
