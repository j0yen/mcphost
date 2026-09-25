//! PRD-mcphost-sqlite-busy-timeout-audit
//! AC9 — Given the alerting source registry is present and
//! `db_busy_total` rises by 10 within 5 min, When the minute tick runs,
//! Then one `db.contention` alert is raised.
//!
//! "The alerting source registry is present" is unconditionally true
//! (`AppState::alerts`, `alerts::AlertRegistry`, is always constructed --
//! see that module's own doc comment on why "no hard dependency" doesn't
//! mean "no registry"). `alerts::tick_once` is the same single
//! deterministic step `alerts::spawn_tick` runs every 60s in production,
//! called directly here instead of waiting out that cadence (same
//! convention as `triggers::tick_once`/`retention`'s
//! `spawn_prune_scheduler_for_test`). `Db::bump_busy_total_for_test` seeds
//! the rise directly rather than driving 10 real lock timeouts.

use crate::common;

#[tokio::test]
async fn busy_total_rise_of_ten_raises_one_contention_alert() {
    let server = common::TestServer::start().await;

    // Baseline tick: establishes the window's first sample at the current
    // (zero) busy_total.
    mcphost::alerts::tick_once(&server.state);
    assert!(server.state.alerts.events().is_empty(), "no rise yet, no alert");

    server.state.db.bump_busy_total_for_test(mcphost::db::ROLE_SERVER, 10);
    mcphost::alerts::tick_once(&server.state);

    let events = server.state.alerts.events();
    assert_eq!(events.len(), 1, "exactly one alert must be raised: {events:?}");
    assert_eq!(events[0].key, "db.contention", "{events:?}");

    // A further tick with no additional rise (busy_total unchanged, and
    // the seeded rise is still within the 5-minute window) must not raise
    // a second alert for the same episode.
    mcphost::alerts::tick_once(&server.state);
    let events = server.state.alerts.events();
    assert_eq!(events.len(), 1, "a still-elevated tick must not re-raise: {events:?}");
}
