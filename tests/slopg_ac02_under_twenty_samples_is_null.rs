//! PRD-mcphost-slo-targets-on-status AC2 (P0) — Given fewer than 20 rows in
//! the window, When `/status.json` is fetched, Then that key's `measured` and
//! `met` are `null` and `sample_n` is the row count.

use crate::common::{TestServer, signup};
use serde_json::Value;

async fn slo(server: &TestServer) -> Value {
    let body: Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");
    body["slo"].clone()
}

#[tokio::test]
async fn slopg_ac02_nineteen_rows_is_null_and_twenty_is_measured() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Slo Few").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    let now = mcphost::state::now_unix();

    for i in 0..19 {
        server.state.db.insert_slo_call_row_for_test(tenant.id, now - 60 - i, 100 + i, true, None, "human").await.expect("row");
    }
    let few = slo(&server).await;
    for key in ["tool_call_p95_ms_24h", "error_rate_24h"] {
        let k = &few[key];
        assert_eq!(k["measured"], Value::Null, "{key} with 19 rows: {k}");
        assert_eq!(k["met"], Value::Null, "{key} met is null, never true: {k}");
        assert_eq!(k["sample_n"].as_i64(), Some(19), "{key} sample_n is the row count: {k}");
    }
    assert_eq!(few.as_object().expect("slo object").len(), 3, "all three keys are always present: {few}");

    // The 20th row crosses the line.
    server.state.db.insert_slo_call_row_for_test(tenant.id, now - 90, 500, true, None, "human").await.expect("row 20");
    let enough = slo(&server).await;
    for key in ["tool_call_p95_ms_24h", "error_rate_24h"] {
        let k = &enough[key];
        assert!(k["measured"].is_number(), "{key} with 20 rows is measured: {k}");
        assert_eq!(k["met"], Value::Bool(true), "{key}: {k}");
        assert_eq!(k["sample_n"].as_i64(), Some(20));
    }
}

#[tokio::test]
async fn slopg_ac02_availability_under_twenty_samples_is_null() {
    let server = TestServer::start().await;
    let day = mcphost::state::rfc3339_from_unix(mcphost::state::now_unix() - 86_400)[..10].to_string();
    server.state.db.upsert_status_daily("mcp".to_string(), day, 10, 12, 5).await.expect("daily row");
    let block = slo(&server).await;
    let a = &block["availability_30d"];
    assert_eq!(a["measured"], Value::Null, "{a}");
    assert_eq!(a["met"], Value::Null, "{a}");
    assert_eq!(a["sample_n"].as_i64(), Some(12), "{a}");
}
