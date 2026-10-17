//! PRD-mcphost-performance-guarantees-measured
//! AC7 (P1) -- Given a keyed free tenant, When it calls `host.whoami`, Then
//! `perf_measured.echo.p95_ms` is an integer or null and
//! `limits.plan.perf_echo_p95_ms == 150`.

use crate::common::{self, McpClient, TestServer, signup};
use mcphost::db::PerfSample;
use serde_json::{Value, json};

#[tokio::test]
async fn pgm_ac07_whoami_carries_target_and_latest_measurement() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Pgm AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let who = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(who["limits"]["plan"]["perf_echo_p95_ms"], json!(150), "{who}");
    assert_eq!(who["limits"]["plan"]["perf_python_cold_p95_ms"], json!(1500), "{who}");
    assert_eq!(who["limits"]["plan"]["perf_http_p95_ms"], json!(800), "{who}");
    assert_eq!(who["perf_measured"]["echo"]["p95_ms"], Value::Null, "no rows yet: {who}");

    server
        .state
        .db
        .insert_perf_sample(PerfSample { ts: 10, plan: "free".into(), path: "echo".into(), p50_ms: 20, p95_ms: 38, n: 20 })
        .await
        .expect("insert");
    let who = common::extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert!(who["perf_measured"]["echo"]["p95_ms"].is_i64(), "{who}");
    assert_eq!(who["perf_measured"]["echo"]["p95_ms"], json!(38), "{who}");
}
