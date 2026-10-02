//! PRD-mcphost-test-suite-flake-lints
//! AC5 (P0) — Given two tests in one binary that set the same variable
//! through `EnvGuard` concurrently, When they run 20 times, Then both pass
//! every time (the race fixture from PR #79 reproduced then guarded).
//!
//! `cargo test`'s default (no nextest) model runs every `#[tokio::test]` fn
//! in a `tests/suite_*.rs` binary concurrently, each on its own thread --
//! exactly the shape that raced `MCPHOST_ADVISORY_MODE` in PR #79 (4f50410):
//! two tests, same process-wide env var, no shared lock. `racer_a`/`racer_b`
//! below are that same shape, guarded by `common::EnvGuard` this time; the
//! driver test re-runs just the two of them, in a fresh child process (so
//! each attempt starts from a clean slate), 20 times and asserts both pass
//! every single time -- not just once, since a race that only sometimes
//! loses would still pass most individual runs.

use crate::common;

/// Widens the race window between "set" and "read back" so an unguarded
/// version of this test would actually lose the race against its sibling
/// often, not just in theory -- same technique `egress_proxy_lock`'s (now
/// deleted) doc comment and PR #79's own grounding incident describe.
const RACE_WINDOW: std::time::Duration = std::time::Duration::from_millis(3);

#[tokio::test]
async fn flakelint_ac05_racer_a_sets_and_reads_back_its_own_value() {
    for _ in 0..10 {
        let _guard = common::EnvGuard::set("MCPHOST_FLAKELINT_AC05_RACE", "a").await;
        tokio::time::sleep(RACE_WINDOW).await;
        assert_eq!(
            std::env::var("MCPHOST_FLAKELINT_AC05_RACE").as_deref(),
            Ok("a"),
            "racer_a must never observe racer_b's value while holding the guard"
        );
    }
}

#[tokio::test]
async fn flakelint_ac05_racer_b_sets_and_reads_back_its_own_value() {
    for _ in 0..10 {
        let _guard = common::EnvGuard::set("MCPHOST_FLAKELINT_AC05_RACE", "b").await;
        tokio::time::sleep(RACE_WINDOW).await;
        assert_eq!(
            std::env::var("MCPHOST_FLAKELINT_AC05_RACE").as_deref(),
            Ok("b"),
            "racer_b must never observe racer_a's value while holding the guard"
        );
    }
}

/// The filter substring that selects exactly the two racer tests above
/// (and nothing else in a ~1000-test suite binary) regardless of which
/// `tests/suite_*.rs` binary `scripts/gen-test-suites.sh` bundles this file
/// into, since libtest's filter matches anywhere in the qualified
/// `modpath::fnname` string.
const RACER_FILTER: &str = "flakelint_ac05_racer";

#[test]
fn both_racers_pass_every_time_across_twenty_runs() {
    let exe = std::env::current_exe().expect("this test binary's own path");
    for attempt in 0..20 {
        let output = std::process::Command::new(&exe)
            .args([RACER_FILTER, "--test-threads", "2"])
            .output()
            .expect("re-run the two racer tests in a fresh child process");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("2 passed; 0 failed"),
            "race attempt {attempt}/20 did not show both racers passing:\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
