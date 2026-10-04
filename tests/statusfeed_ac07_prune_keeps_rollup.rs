//! PRD-mcphost-status-feed AC7 (P0): given samples older than 90 days, when the
//! daily prune runs, then they are deleted and the rollup rows for those
//! days remain.

use crate::common;

use common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};

#[tokio::test]
async fn prune_deletes_old_samples_but_keeps_the_rollup() {
    let server = TestServer::start().await;

    // Anchored to noon UTC of the day 120 days ago, not to "now"'s own
    // time-of-day: the 10 samples below are spread one minute apart (up
    // to 9 minutes past `old_ts`), and `now_unix() - 120 * 86_400` would
    // inherit whatever time-of-day the test happens to run at. When that
    // falls within the last 9 minutes of a UTC day, the spread straddles
    // the day boundary, so `rollup_day` (bounded by `day_bounds`'s exact
    // UTC midnight-to-midnight window) only picks up the samples before
    // midnight -- an intermittent short rollup (`total_samples` < 10)
    // that looks like a prune bug but is really the fixture's choice of
    // timestamp. Noon UTC keeps the whole 9-minute spread comfortably
    // inside one calendar day regardless of wall-clock time at test run.
    let old_day_start = now_unix().div_euclid(86_400) * 86_400 - 120 * 86_400;
    let old_ts = old_day_start + 12 * 3_600;
    let old_day = rfc3339_from_unix(old_ts)[..10].to_string();
    for i in 0..10 {
        server
            .state
            .db
            .insert_status_sample("mcp".to_string(), old_ts + i * 60, true, 5, "self".to_string())
            .await
            .expect("insert old sample");
    }
    mcphost::statusfeed::rollup_day(&server.state, "mcp", &old_day)
        .await
        .expect("rollup old day");

    let recent_ts = now_unix() - 60;
    server
        .state
        .db
        .insert_status_sample("mcp".to_string(), recent_ts, true, 5, "self".to_string())
        .await
        .expect("insert recent sample");

    let deleted = mcphost::statusfeed::prune_once(&server.state)
        .await
        .expect("prune_once");
    assert_eq!(deleted, 10, "prune should only remove the 10 old samples");

    let (ok, total) = server
        .state
        .db
        .status_samples_ok_ratio_since("mcp".to_string(), 0)
        .await
        .expect("status_samples_ok_ratio_since");
    assert_eq!(total, 1, "only the recent sample should remain");
    assert_eq!(ok, 1);

    let rollup_rows = server
        .state
        .db
        .status_daily_recent("mcp".to_string(), 365)
        .await
        .expect("status_daily_recent");
    assert!(
        rollup_rows.iter().any(|r| r.day == old_day && r.total_samples == 10),
        "rollup row for {old_day} should survive the prune: {rollup_rows:?}"
    );
}
