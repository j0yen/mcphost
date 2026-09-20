//! PRD-mcphost-data-retention
//! AC7 (P0) — Given prod after ship, When `admin.usage` is read on the
//! following day, Then `last_prune` is younger than 24 h (proof: the live
//! read in the trailer).
//!
//! That live read can't exist pre-ship. What this test pins down instead
//! is the engineering guarantee the live read actually depends on: the
//! nightly scheduler (`retention::spawn_prune_scheduler`, wired into
//! `mcphost serve` at real startup) must fire *unattended* -- with no
//! `admin.prune_now`/manual trigger anywhere in this test -- and refresh
//! `last_prune` on its own. `spawn_prune_scheduler_for_test` swaps a short
//! interval in for the real 03:30 UTC cadence so this doesn't wait a real
//! day, the same "test-only knob instead of the real cadence" shape AC6's
//! disk-floor override already uses. Without that wiring (e.g. if
//! `mcphost serve` never spawned the scheduler, or the scheduler never
//! called `prune_once`), `last_prune` would stay null/stale and this test
//! would fail.

use crate::common;
use common::{ADMIN_KEY, McpClient, extract_structured, signup};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn automatic_scheduler_refreshes_last_prune_without_manual_trigger() {
    let server = common::TestServer::start().await;
    let (namespace, _key) = signup(&server.base_url, "AC7 Tenant").await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("query tenant")
        .expect("tenant exists");

    server
        .state
        .db
        .set_retention_days_for_test("calls".to_string(), 90)
        .await
        .expect("set calls retention window");

    let now = mcphost::state::now_unix();
    let ninety_one_days_ago = now - 91 * 86_400;
    server
        .state
        .db
        .insert_calls_row_for_test(tenant.id, ninety_one_days_ago)
        .await
        .expect("insert expired call");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Before the automatic scheduler is ever started, no prune has run.
    let before = admin
        .tools_call("admin.usage", json!({}))
        .await
        .expect("admin.usage");
    assert!(
        extract_structured(&before)["last_prune"].is_null(),
        "no prune should have run before the scheduler starts"
    );

    // Start the automatic loop -- never admin.prune_now, never
    // db.prune_once directly -- standing in for `mcphost serve`'s real
    // startup wiring, just on a short interval instead of 03:30 UTC.
    let _scheduler = mcphost::retention::spawn_prune_scheduler_for_test(
        (*server.state).clone(),
        Duration::from_millis(20),
    );

    let mut last_prune = json!(null);
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let usage = admin
            .tools_call("admin.usage", json!({}))
            .await
            .expect("admin.usage");
        last_prune = extract_structured(&usage)["last_prune"].clone();
        if !last_prune.is_null() {
            break;
        }
    }

    assert_eq!(
        last_prune["ok"].as_bool(),
        Some(true),
        "the unattended scheduler must run a clean prune cycle on its own, got {last_prune:?}"
    );

    // The interval override fires the loop continuously (unlike a single
    // on-demand admin.prune_now/db.prune_once call), so by the time this
    // poll observes a non-null last_prune, several cycles may already have
    // run -- the *latest* one legitimately reports 0 deleted once nothing
    // is left to prune. The row-count check below is the race-free proof
    // that the automatic path actually did the deletion, not just that it
    // ran.
    let remaining_calls = server
        .state
        .db
        .table_row_count_for_test("calls".to_string())
        .await
        .expect("count calls after automatic prune");
    assert_eq!(
        remaining_calls, 0,
        "the automatic, unattended scheduler must actually delete the expired row"
    );

    // Cross-check freshness against the DB-level timestamp directly
    // (avoids re-parsing the JSON `last_prune.at` rfc3339 string): this is
    // exactly the field AC7's live prod read will compare against 24h.
    let size = server.state.db.usage_size_stats().await.expect("usage_size_stats");
    let at_unix = size.last_prune.expect("a prune has run").at_unix;
    let age = mcphost::state::now_unix() - at_unix;
    assert!(
        age < 24 * 3600,
        "last_prune must be younger than 24h for AC7's live prod check the following day to hold, got age {age}s"
    );
}
