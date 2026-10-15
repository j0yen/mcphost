//! PRD-mcphost-incident-banner-for-agents
//! AC1 — Given a fake-mode handler where 20 of the next 100 results are
//! injected `storage` failures inside one 5-minute window, When the window
//! evaluates, Then an incident titled `auto:error_rate storage 20.0%` is
//! open and the next `storage` error carries
//! `data.incident.{id, since, status_url, retry_after_s}` with
//! `retry_after_s == 30`.

use crate::common;
use common::TestServer;
use mcphost::errors::AppError;
use mcphost::incident_banner::{evaluate_error_rate, record_result};

#[tokio::test]
async fn incban_ac01() {
    let server = TestServer::start().await;
    let base = server.state.public_url.clone();
    let now = mcphost::state::now_unix();

    // Below the open rule: 19 of 100 is not enough results.
    for i in 0..100 {
        record_result(&base, now - (i % 200), (i < 19).then_some("storage"));
    }
    evaluate_error_rate(&server.state, now).await.expect("evaluate");
    assert!(server.state.db.list_open_incidents().await.unwrap().is_empty(), "19 faults must not open");
    let quiet = AppError::Storage("disk".into()).into_error_data_at(Some(&base));
    assert!(quiet.data.unwrap().get("incident").is_none(), "no incident yet, no banner");

    // The 20th storage failure in the same window crosses both thresholds.
    record_result(&base, now, Some("storage"));
    evaluate_error_rate(&server.state, now).await.expect("evaluate");
    let open = server.state.db.list_open_incidents().await.unwrap();
    assert_eq!(open.len(), 1, "exactly one incident: {open:?}");
    assert_eq!(open[0].title, "auto:error_rate storage 19.8%");
}

#[tokio::test]
async fn incban_ac01_exact_title_and_banner() {
    let server = TestServer::start().await;
    let base = server.state.public_url.clone();
    let now = mcphost::state::now_unix();

    for i in 0..100 {
        record_result(&base, now - (i % 250), (i % 5 == 0).then_some("storage"));
    }
    evaluate_error_rate(&server.state, now).await.expect("evaluate");

    let open = server.state.db.list_open_incidents().await.unwrap();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].title, "auto:error_rate storage 20.0%");

    // A second evaluation never opens a duplicate.
    evaluate_error_rate(&server.state, now).await.expect("evaluate");
    assert_eq!(server.state.db.list_open_incidents().await.unwrap().len(), 1);

    let data = AppError::Storage("disk".into()).into_error_data_at(Some(&base)).data.expect("data");
    let incident = data.get("incident").expect("data.incident on a storage error");
    assert_eq!(incident["id"], open[0].id);
    assert!(incident["since"].as_str().is_some_and(|s| s.contains('T')), "RFC 3339 since: {incident}");
    assert_eq!(incident["status_url"], format!("{base}/status.html#{}", open[0].id));
    assert_eq!(incident["retry_after_s"], 30);
}
