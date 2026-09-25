//! PRD-mcphost-abuse-guard-ban-list
//! AC10 (P1) — Given 10 000 active bans, When 1 000 requests run, Then
//! p99 added latency from enforcement is < 1 ms (in-memory cache).
//!
//! Enforcement's whole per-request cost is [`mcphost::bans::BanCache::check`]
//! -- one `HashMap` lookup, no query -- so this measures that call directly
//! against a cache freshly refreshed from 10 000 real `bans` rows, the
//! same [`mcphost::bans::BanCache::refresh`] every write and the 30s tick
//! call in production.

use crate::common;
use common::TestServer;

#[tokio::test]
async fn cache_lookup_p99_latency_stays_under_a_millisecond_at_ten_thousand_bans() {
    let server = TestServer::start().await;
    let now = mcphost::state::now_unix();

    server
        .state
        .db
        .bulk_insert_addr_bans_for_test(10_000, now + 3600)
        .await
        .expect("seed 10k bans");
    server.state.bans.refresh(&server.state.db).await.expect("refresh cache");

    // 1000 lookups against a mix of addresses -- some inside the seeded
    // `10.x.x.x` banned range, most in an unbanned range, matching the
    // overwhelmingly-unbanned traffic mix enforcement actually sees.
    let mut durations = Vec::with_capacity(1000);
    for i in 0..1000i64 {
        let addr = if i % 10 == 0 {
            format!("10.{}.{}.{}", (i >> 16) & 0xff, (i >> 8) & 0xff, i & 0xff)
        } else {
            format!("203.0.{}.{}", (i >> 8) & 0xff, i & 0xff)
        };
        let start = std::time::Instant::now();
        let _ = server.state.bans.check("addr", &addr);
        durations.push(start.elapsed());
    }

    durations.sort();
    let p99 = durations[989];
    assert!(
        p99 < std::time::Duration::from_millis(1),
        "p99 cache-check latency over 1000 lookups against 10k bans was {p99:?}, expected < 1ms"
    );
}
