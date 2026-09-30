//! PRD-mcphost-session-bound-tenant-after-signup
//! AC8 (P0) — Given 50,000 sessions each running `signup` under the plan's
//! rate limit lifted for the test, When the flood ends, Then the binding map
//! holds at most 10,000 entries and the process RSS growth attributable to
//! it is under 20 MB.
//!
//! The 50,000-session half drives `SessionBindings` directly -- it is the
//! exact object `AppState::session_bindings` holds and the exact API
//! `handler::bind_session_to_created_tenant` calls, so the cap and the
//! eviction order under test are production's own. Fifty thousand real HTTP
//! signups would spend the whole suite's time budget writing 50,000 tenant
//! rows to SQLite, which measures the database, not requirement 4's bound.
//! The second test closes that gap from the other end: real signups over the
//! wire land real entries in the server's own map.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::session_bind::{BINDING_IDLE_TTL_SECS, MAX_BOUND_SESSIONS, SessionBindings};
use serde_json::json;

/// Resident set size in bytes, from this process's own `/proc/self/statm`
/// (field 2 is resident pages). Linux-only, which is what the runner and
/// prod both are.
fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
    let resident_pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .expect("statm has a resident field")
        .parse()
        .expect("resident pages is a number");
    resident_pages * 4096
}

/// The full name of the flood test, as `libtest` spells it -- used to
/// re-run exactly this one test in a child process.
const FLOOD_TEST_NAME: &str =
    "sessbind_ac08_binding_map_is_bounded_under_a_session_flood::a_flood_of_50000_sessions_leaves_at_most_10000_bindings_under_20mb";

/// The env var that tells a re-exec'd copy of this binary it is the child.
const CHILD_MARKER: &str = "SESSBIND_AC08_FLOOD_CHILD";

const FLOOD: usize = 50_000;
const RSS_BUDGET_BYTES: u64 = 20 * 1024 * 1024;

/// Runs the flood and returns `(entries, rss_growth_bytes)`.
fn run_flood() -> (usize, u64) {
    let bindings = SessionBindings::new();
    // Warm the allocator before measuring, so the baseline is this process's
    // steady state rather than its first-touch page faults.
    for tenant_id in 0..1_000i64 {
        bindings.bind(&bindings.issue(), tenant_id, 0);
    }
    let baseline_rss = rss_bytes();

    let mut last = String::new();
    for tenant_id in 0..FLOOD as i64 {
        last = bindings.issue();
        assert!(bindings.bind(&last, tenant_id, 0), "every minted id must bind");
    }
    assert_eq!(
        bindings.lookup(&last, 0),
        Some(FLOOD as i64 - 1),
        "the most recent session must be the one that survived eviction"
    );
    (bindings.len(), rss_bytes().saturating_sub(baseline_rss))
}

#[tokio::test]
async fn a_flood_of_50000_sessions_leaves_at_most_10000_bindings_under_20mb() {
    // The RSS half of this AC is a *process*-level measurement, and this
    // suite binary runs ~980 tests across many threads at once -- reading
    // /proc/self/statm in the middle of that measures the whole suite, not
    // the flood. So the parent re-runs exactly this one test in a child
    // process of the same binary, where the flood is the only thing running,
    // and asserts on what the child reports.
    if std::env::var(CHILD_MARKER).is_ok() {
        let (entries, growth) = run_flood();
        assert_eq!(
            entries, MAX_BOUND_SESSIONS,
            "requirement 4: the binding map holds at most 10,000 entries under a \
             {FLOOD}-session flood"
        );
        assert!(
            growth < RSS_BUDGET_BYTES,
            "requirement 4: RSS growth attributable to the binding map must stay under 20 MB, \
             saw {growth} bytes across {FLOOD} sessions"
        );
        return;
    }

    let exe = std::env::current_exe().expect("this test binary's own path");
    let output = std::process::Command::new(exe)
        .args(["--exact", FLOOD_TEST_NAME, "--test-threads", "1", "--nocapture"])
        .env(CHILD_MARKER, "1")
        .output()
        .expect("re-run this test alone in a child process");
    assert!(
        output.status.success(),
        "the isolated flood run failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the child must have actually run the flood: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    // The entry cap itself needs no isolation, so it is also asserted here
    // in-process: a regression that removed eviction fails both runs.
    let (entries, _) = run_flood();
    assert_eq!(
        entries, MAX_BOUND_SESSIONS,
        "requirement 4: the binding map holds at most 10,000 entries under a {FLOOD}-session flood"
    );
}

/// Requirement 4's other clause: a binding is dropped once its session has
/// been idle for 24 h, so a flood that stops does not hold memory forever.
#[test]
fn an_idle_session_stops_counting_against_the_map() {
    let bindings = SessionBindings::new();

    // A session used just under the idle window stays bound...
    let busy = bindings.issue();
    assert!(bindings.bind(&busy, 1, 1_000));
    let last_touch = 1_000 + BINDING_IDLE_TTL_SECS - 1;
    assert_eq!(bindings.lookup(&busy, last_touch), Some(1));
    // ...and each use restarts its 24 h, which is what "idle expiry" means:
    // a full window measured from the LAST use, not from the signup.
    assert_eq!(bindings.lookup(&busy, last_touch + BINDING_IDLE_TTL_SECS - 1), Some(1));

    // A session that is never touched again is gone once the window passes.
    let idle = bindings.issue();
    assert!(bindings.bind(&idle, 2, 1_000));
    assert_eq!(
        bindings.lookup(&idle, 1_000 + BINDING_IDLE_TTL_SECS),
        None,
        "a binding idle for 24 h is gone"
    );
    assert_eq!(bindings.len(), 1, "only the still-active session remains");
}

/// The wire half: real signups on distinct sessions land real entries in the
/// running server's own map, and a signup that returns a handoff token
/// instead of a key binds nothing (requirement 5 -- the connection that
/// redeems it is the one that gets the binding).
#[tokio::test]
async fn real_signups_land_in_the_running_servers_own_map() {
    let server = TestServer::start_with_signup_rate_limit(100).await;
    assert_eq!(server.state.session_bindings.len(), 0);

    for i in 0..5 {
        let session = McpClient::new(&server.base_url).with_session_continuity();
        session
            .tools_call("signup", json!({"name": format!("AC8 Tenant {i}")}))
            .await
            .expect("signup");
        assert_eq!(
            server.state.session_bindings.len(),
            i + 1,
            "each session that signs up adds exactly one binding"
        );
    }

    // A connection that never signs up never earns an entry, however many
    // requests it makes.
    let anonymous = McpClient::new(&server.base_url).with_session_continuity();
    for _ in 0..10 {
        let _ = anonymous.tools_call("host.catalog.search", json!({})).await;
    }
    assert_eq!(
        server.state.session_bindings.len(),
        5,
        "requests alone must never grow the map -- only a signup/redeem does"
    );

    // Handoff mode hands back no key on this connection, so it binds nothing.
    let handoff_session = McpClient::new(&server.base_url).with_session_continuity();
    handoff_session
        .tools_call("signup", json!({"name": "AC8 Handoff Tenant", "handoff": true}))
        .await
        .expect("handoff signup");
    assert_eq!(
        server.state.session_bindings.len(),
        5,
        "a handoff signup returns a token, not a key, so it binds nothing yet"
    );
}
