//! PRD-mcphost-msg-wait-long-poll-contract
//! AC1 (P0) — Given no `tenant_key` and a fresh connection, When
//! `host.msg.wait` is called with default args, Then the response arrives in
//! < 1,000 ms with `timed_out: true`, `messages: []` and an `onboarding`
//! field.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn keyless_msg_wait_returns_at_once_with_timed_out_and_onboarding() {
    // `perf_budget!` runs the body once untimed, then PERF_BUDGET_SAMPLES
    // times, and every run mints a tenant by implicit signup from 127.0.0.1.
    // The server's per-address signup cap admits exactly those runs; at the
    // default cap (5 an hour) the sixth was refused `signup_rate_limited`
    // whenever the budget was measured (`MCPHOST_PERF_SKIP` unset).
    let signups = 1 + common::PERF_BUDGET_SAMPLES as i64;
    let server = TestServer::start_with_signup_rate_limit(signups).await;

    // Each sample is a fresh connection, so every call is a genuine key-less
    // first call; the median over the warm samples must be under 1,000 ms.
    crate::perf_budget!(1000, {
        let session = McpClient::new(&server.base_url).with_session_continuity();
        let result = session
            .tools_call("host.msg.wait", json!({}))
            .await
            .expect("a key-less host.msg.wait must succeed via implicit signup");
        let body = extract_structured(&result);

        assert_eq!(body["timed_out"], json!(true), "{body}");
        assert_eq!(body["messages"], json!([]), "{body}");
        assert!(body["waited_ms"].as_u64().is_some_and(|ms| ms < 1000), "{body}");
        assert!(body["onboarding"]["tenant"].is_string(), "onboarding must be present: {body}");
    });
}
