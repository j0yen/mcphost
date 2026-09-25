//! PRD-mcphost-alerting-webhook
//! AC5 — Given the webhook server returns 503 three times, When delivery
//! runs, Then three retries occur at the documented backoff,
//! `delivery_status` is `"failed"`, and the host keeps serving tool calls.

use crate::common;
use common::TestServer;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[tokio::test]
async fn three_retries_exhaust_then_failed_and_host_still_serves() {
    let mock = MockServer::start().await;
    // Each attempt's arrival time is recorded here (rather than trusting
    // `mock.received_requests()`, whose `Request` carries no timestamp) so
    // this test can prove the gap between consecutive attempts matches
    // each of `retry_backoff_ms`'s own (distinct) per-attempt values --
    // not just that *some* delay happened.
    let attempt_times: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let attempt_times_recorder = attempt_times.clone();
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .respond_with(move |_: &Request| {
            attempt_times_recorder.lock().unwrap().push(Instant::now());
            ResponseTemplate::new(503)
        })
        .mount(&mock)
        .await;

    // requirement 3's documented backoff is 1s/5s/25s; shrunk here (a
    // field on AlertConfig, never process env -- tests run in parallel) so
    // this test proves the retry COUNT and final status without a real
    // ~31s wait for the full sequence to exhaust. The three values are
    // kept distinct (not e.g. [10,10,10]) so the per-attempt timing
    // assertion below can tell "honors `retry_backoff_ms`" apart from "just
    // sleeps some flat amount every time".
    let retry_backoff_ms: [u64; 3] = [30, 80, 150];
    let alert_config = mcphost::alerts::AlertConfig {
        webhook_url: Some(format!("{}/alerts", mock.uri())),
        retry_backoff_ms,
        ..mcphost::alerts::AlertConfig::default()
    };
    let server = TestServer::start_with_alert_config(alert_config).await;

    // Signed up before the pause takes effect -- `signup_pause` only gates
    // `signup` itself (technical considerations), so this client's own
    // tool calls below prove the host keeps serving despite it.
    let (_ns, key) = common::signup(&server.base_url, "Still Serving Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    std::fs::write(server.state.signup_pause.path(), "maintenance\n").expect("write pause file");

    let mut failed_row = None;
    for _ in 0..60 {
        if let Ok(Some(row)) = server
            .state
            .db
            .most_recent_alert_for_key("signup.paused".to_string())
            .await
            && row.delivery_status == "failed"
        {
            failed_row = Some(row);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let row = failed_row.expect("delivery_status reached failed within the test's own bound");
    assert_eq!(row.delivery_status, "failed");
    assert!(row.delivered_at.is_none(), "a failed delivery never sets delivered_at");

    // requirement 3: one initial attempt plus 3 retries -- 4 total.
    let requests = mock.received_requests().await.expect("mock records requests");
    assert_eq!(requests.len(), 4, "one initial attempt plus 3 documented retries");

    // AC5's "at the documented backoff" clause: the gap before each retry
    // must be at least that retry's own `retry_backoff_ms` entry --
    // `tokio::time::sleep` never returns early, so a real implementation
    // clears this bound comfortably, while one that ignores
    // `retry_backoff_ms` (e.g. sleeps 0ms, or reuses a single flat delay
    // for every attempt) fails it.
    let stamps = attempt_times.lock().unwrap().clone();
    assert_eq!(stamps.len(), 4, "one timestamp recorded per attempt");
    let gaps: Vec<Duration> = (0..3)
        .map(|i| stamps[i + 1].saturating_duration_since(stamps[i]))
        .collect();
    for (i, gap) in gaps.iter().enumerate() {
        assert!(
            *gap >= Duration::from_millis(retry_backoff_ms[i]),
            "retry {} must wait at least its documented backoff ({}ms), got {gap:?}",
            i + 1,
            retry_backoff_ms[i],
        );
    }
    // Catches an implementation that sleeps a single flat delay (e.g. the
    // largest configured value) for every retry instead of walking through
    // `retry_backoff_ms` in order -- the per-attempt gap must actually grow
    // the way the documented 1s/5s/25s shape (and this test's own strictly
    // increasing [30,80,150]) does.
    assert!(
        gaps[0] < gaps[2],
        "backoff must escalate across retries like the documented 1s/5s/25s, \
         got first gap {:?} >= third gap {:?}",
        gaps[0],
        gaps[2],
    );

    // The host must still be serving ordinary tool calls after a webhook
    // sink that never succeeds.
    client
        .tools_call(
            "host.tool_publish",
            serde_json::json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("the host must keep serving tool calls after a failed alert delivery");
}
