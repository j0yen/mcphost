//! AC11 (P1) — Given three ledgered batches, When an admin calls
//! `admin.meter_status`, Then it reports last batch span, current lag, and
//! per-tenant emitted counts for the month.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;
use serde_json::json;

#[tokio::test]
async fn meter_status_reports_last_batch_lag_and_monthly_totals() {
    let server = TestServer::start().await;
    let (_ns_a, _key_a, tenant_a) =
        signup_and_make_pro(&server, "Meter Status A", "cus_ac11_a").await;
    let (_ns_b, _key_b, tenant_b) =
        signup_and_make_pro(&server, "Meter Status B", "cus_ac11_b").await;
    let fake = FakeBillingClient::new(mcphost::state::now_unix());

    // Batch 1: tenant A only.
    record_ok_calls(&server, tenant_a, "some_tool", 10).await;
    let batch1 = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("batch 1");
    assert_eq!(batch1.calls_covered, 10);

    // Batch 2: tenant B only.
    record_ok_calls(&server, tenant_b, "some_tool", 5).await;
    let batch2 = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("batch 2");
    assert_eq!(batch2.calls_covered, 5);

    // Batch 3: tenant A again -- this is the batch admin.meter_status's
    // last_batch must report.
    record_ok_calls(&server, tenant_a, "some_tool", 7).await;
    let batch3 = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("batch 3");
    assert_eq!(batch3.calls_covered, 7);
    let group3 = &batch3.groups[0];

    // Some pending, unemitted calls -- meter_status's meter_lag must count
    // these without a fourth `run_once`.
    record_ok_calls(&server, tenant_a, "some_tool", 3).await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.meter_status", json!({}))
        .await
        .expect("admin.meter_status");
    let status = common::extract_structured(&result);

    let last_batch = &status["last_batch"];
    assert_eq!(last_batch["batch_id"], json!(batch3.batch_id));
    assert_eq!(last_batch["first_call_id"], json!(group3.first_call_id));
    assert_eq!(last_batch["last_call_id"], json!(group3.last_call_id));
    assert_eq!(last_batch["count"], json!(7));

    assert_eq!(status["meter_lag"], json!(3), "the 3 unemitted calls just recorded");

    let by_tenant = status["emitted_this_month"]
        .as_array()
        .expect("emitted_this_month array");
    assert_eq!(by_tenant.len(), 2, "{by_tenant:?}");
    // Newest/largest emitter first: tenant A (10 + 7 = 17) before tenant B (5).
    assert_eq!(by_tenant[0]["count"], json!(17));
    assert_eq!(by_tenant[1]["count"], json!(5));
}

#[tokio::test]
async fn meter_status_reports_no_batch_and_zero_lag_when_nothing_ever_ledgered() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let result = admin
        .tools_call("admin.meter_status", json!({}))
        .await
        .expect("admin.meter_status");
    let status = common::extract_structured(&result);
    assert!(status["last_batch"].is_null());
    assert_eq!(status["meter_lag"], json!(0));
    assert_eq!(status["emitted_this_month"], json!([]));
}
