//! PRD-mcphost-docs-telemetry-failed-after-search AC7 (P1) — Given rows 31
//! days old, When the retention sweep runs, Then they are gone and the sweep
//! log line reports `help_search_events=<n>`.

use crate::common::TestServer;
use mcphost::state::now_unix;

#[tokio::test]
async fn dtf_ac07_sweep_removes_31_day_old_rows_and_reports_the_count() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = now_unix();
    for i in 0..3 {
        db.insert_help_search_event_for_test(Some(1), "old query", None, now - 31 * 86_400 - i, Some("invalid_spec"))
            .await
            .expect("seed old");
    }
    db.insert_help_search_event_for_test(Some(1), "fresh query", None, now - 29 * 86_400, None).await.expect("seed fresh");

    let report = db.prune_once().await.expect("prune");
    assert_eq!(report.deleted.get("help_search_events"), Some(&3), "{:?}", report.deleted);
    let line = mcphost::retention::sweep_log_line(&report.deleted);
    assert!(line.contains("help_search_events=3"), "{line}");

    let rows = db.help_search_events_for_test().await.expect("rows");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, "fresh query");
}
