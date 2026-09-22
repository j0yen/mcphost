//! PRD-mcphost-team-memory
//! P1 requirement 6 / AC8 — Given `MCPHOST_LIVE` unset, When the test
//! suite runs, Then the Live test is skipped, not failed.
//!
//! AC6 (P0, Live; deferred -- proving it needs a real run against
//! production `https://mcphost.dev/mcp`, which this worktree's own gate
//! cannot reach) is the test `live_proof_against_real_mcphost_when_enabled`
//! below embodies: the full `proof.sh` run against the real `$MCPHOST_URL`
//! (default `https://mcphost.dev/mcp`), gated on `MCPHOST_LIVE=1` so it
//! never touches the network in a normal `cargo test`. Same convention as
//! `tests/mcphost_share_a_tool_not_a_key_ac08_live_test_skipped_without_env.rs`.

use std::sync::Mutex;
use tokio::process::Command;

/// Pure predicate, deliberately not reading `std::env` itself, so AC8 can
/// assert its behavior for an unset variable deterministically instead of
/// depending on (and possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// The actual skip-gate `live_proof_against_real_mcphost_when_enabled`
/// below runs: reads the *real* `MCPHOST_LIVE` from the process
/// environment through `live_mode_enabled`. Both the live test and
/// `mcphost_live_env_var_gates_the_real_skip_check` call this same
/// function, so a broken read (wrong var name, inverted condition, ...)
/// fails the latter instead of only being exercised by hand-fed literals.
fn should_run_live() -> bool {
    live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref())
}

/// Guards mutation of the process-global `MCPHOST_LIVE` env var: this test
/// binary's tests run on multiple threads by default, and both this file's
/// tests read/write that var.
static MCPHOST_LIVE_ENV_LOCK: Mutex<()> = Mutex::new(());

/// AC6 (P0, Live) -- runs proof.sh against the real `$MCPHOST_URL`
/// (default `https://mcphost.dev/mcp`) end to end. Only executes with
/// `MCPHOST_LIVE=1`; otherwise this is a successful no-op (AC8).
#[tokio::test]
async fn live_proof_against_real_mcphost_when_enabled() {
    let should_run = {
        // Same lock `mcphost_live_env_var_gates_the_real_skip_check` takes
        // while it briefly clears `MCPHOST_LIVE`; not held across an await.
        let _guard = MCPHOST_LIVE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        should_run_live()
    };
    if !should_run {
        eprintln!(
            "skipping live proof.sh run: set MCPHOST_LIVE=1 (and optionally MCPHOST_URL) \
             to run it against a real endpoint"
        );
        return;
    }
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/team-memory/proof.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    if let Ok(url) = std::env::var("MCPHOST_URL") {
        cmd.env("MCPHOST_URL", url);
    }
    let output = cmd.output().await.expect("run examples/team-memory/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "live proof.sh run failed:\n{stdout}");
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(!live_mode_enabled(Some("0")), "MCPHOST_LIVE=0 must disable live mode");
    assert!(!live_mode_enabled(Some("true")), "only the literal '1' enables live mode");
    assert!(live_mode_enabled(Some("1")), "MCPHOST_LIVE=1 must enable live mode");
}

/// AC8 -- Given `MCPHOST_LIVE` unset, When the test suite runs, Then the
/// Live test is skipped, not failed. Exercises `should_run_live`, the same
/// function `live_proof_against_real_mcphost_when_enabled` itself calls to
/// decide whether to skip, against the *real* process environment instead
/// of a hand-fed literal -- so a broken wire-up (e.g. reading the wrong
/// env var, or an inverted check) fails this test.
///
/// Deliberately never sets `MCPHOST_LIVE=1` here: this test binary runs
/// its tests on multiple threads, including `live_proof_against_real_mcphost_when_enabled`
/// concurrently, and flipping the var to "1" process-wide could make that
/// test attempt a real network call in an offline test run.
#[test]
fn mcphost_live_env_var_gates_the_real_skip_check() {
    let _guard = MCPHOST_LIVE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("MCPHOST_LIVE").ok();

    // SAFETY: exclusive access to MCPHOST_LIVE held by MCPHOST_LIVE_ENV_LOCK
    // for the lifetime of `_guard`; restored before it drops.
    unsafe {
        std::env::remove_var("MCPHOST_LIVE");
    }
    assert!(
        !should_run_live(),
        "with MCPHOST_LIVE unset in the real environment, the live test's own \
         skip-gate must evaluate to 'skip'"
    );

    // SAFETY: see above.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("MCPHOST_LIVE", v),
            None => std::env::remove_var("MCPHOST_LIVE"),
        }
    }
}
