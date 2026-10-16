//! PRD-mcphost-incident-banner-for-agents-b
//! AC2 — Given an incident auto-opened by the error-rate trigger, When 10
//! consecutive minutes show a host-fault ratio under 1 %, Then the incident
//! is closed with a timeline entry naming the trigger and the banner
//! disappears.

use crate::common;
use common::TestServer;
use mcphost::incident_banner::{banner_for, evaluate_error_rate, record_result};
use serde_json::Value;

#[tokio::test]
async fn incbanb_ac02() {
    let server = TestServer::start().await;
    let state = &server.state;
    let base = state.public_url.clone();
    mcphost::incident_banner::reset_window_for_tests(&base);
    let t0 = mcphost::state::now_unix();

    for i in 0..100 {
        record_result(&base, t0 - (i % 200), (i % 5 == 0).then_some("storage"));
    }
    evaluate_error_rate(state, t0).await.expect("evaluate");
    let open = state.db.list_open_incidents().await.unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    let id = open[0].id;
    assert!(open[0].title.starts_with("auto:error_rate"), "{}", open[0].title);
    assert!(banner_for(&base).is_some_and(|b| b.id == id), "banner is up while open");

    // The window drains 5 minutes later; every result since is healthy.
    let calm_start = t0 + 301;
    record_result(&base, calm_start, None);
    evaluate_error_rate(state, calm_start).await.expect("evaluate");
    // 9 minutes 59 s of calm: still open.
    let almost = calm_start + 599;
    record_result(&base, almost, None);
    evaluate_error_rate(state, almost).await.expect("evaluate");
    assert_eq!(state.db.list_open_incidents().await.unwrap().len(), 1, "not yet 10 minutes");
    assert!(banner_for(&base).is_some());

    // A fault burst resets the calm streak (consecutive minutes, not total).
    // 1 fault in 2 results is far over 1 %.
    record_result(&base, almost + 1, Some("storage"));
    evaluate_error_rate(state, almost + 1).await.expect("evaluate");
    let after_burst = almost + 301;
    record_result(&base, after_burst, None);
    evaluate_error_rate(state, after_burst).await.expect("evaluate");
    let t_not_yet = after_burst + 599;
    evaluate_error_rate(state, t_not_yet).await.expect("evaluate");
    assert_eq!(state.db.list_open_incidents().await.unwrap().len(), 1, "streak was reset by the burst");

    let t_close = after_burst + 600;
    evaluate_error_rate(state, t_close).await.expect("evaluate");
    assert!(state.db.list_open_incidents().await.unwrap().is_empty(), "closed after 10 calm minutes");
    assert!(banner_for(&base).is_none(), "banner disappears");

    let closed = state.db.find_incident(id).await.unwrap().expect("incident row");
    assert!(closed.closed_at.is_some());
    let timeline: Value = serde_json::from_str(&closed.timeline_json).expect("timeline json");
    let last = timeline.as_array().and_then(|a| a.last()).expect("timeline entries");
    assert_eq!(last["type"], "closed");
    assert!(last["message"].as_str().is_some_and(|m| m.contains("auto:error_rate")), "{last}");

    let body: Value = reqwest::get(format!("{}/status.json", server.base_url)).await.unwrap().json().await.unwrap();
    assert!(body["banner"].is_null(), "{body}");
}
