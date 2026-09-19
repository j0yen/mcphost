//! PRD-mcphost-share-a-tool-not-a-key
//! P1 requirement 6 / AC8 — Given `MCPHOST_LIVE` unset, When the test
//! suite runs, Then the Live test is skipped, not failed.
//!
//! AC6 (P0, Live; deferred -- proving it needs a real run against
//! production `https://mcphost.dev/mcp`, which this worktree's own gate
//! cannot reach) is the test `live_proof_against_real_mcphost_when_enabled`
//! below embodies: the full `proof.sh` run against the real `$MCPHOST_URL`
//! (default `https://mcphost.dev/mcp`), gated on `MCPHOST_LIVE=1` so it
//! never touches the network in a normal `cargo test`. This repo's test
//! runner has no dedicated "skipped" status distinct from "passed" (no
//! `#[ignore]`-by-env idiom existed before this PRD -- see this file's own
//! commit), so "skipped" here means the gated test function returns
//! successfully having done no network I/O, which reads as `ok` in
//! `cargo test` output, never `FAILED`.

use tokio::process::Command;

/// Pure predicate, deliberately not reading `std::env` itself, so AC8 can
/// assert its behavior for an unset variable deterministically instead of
/// depending on (and possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// AC6 (P0, Live) -- runs proof.sh against the real `$MCPHOST_URL`
/// (default `https://mcphost.dev/mcp`) end to end. Only executes with
/// `MCPHOST_LIVE=1`; otherwise this is a successful no-op (AC8).
#[tokio::test]
async fn live_proof_against_real_mcphost_when_enabled() {
    if !live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
        eprintln!(
            "skipping live proof.sh run: set MCPHOST_LIVE=1 (and optionally MCPHOST_URL, \
             UPSTREAM_URL) to run it against a real endpoint"
        );
        return;
    }
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/share-a-tool/proof.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    if let Ok(url) = std::env::var("MCPHOST_URL") {
        cmd.env("MCPHOST_URL", url);
    }
    if let Ok(url) = std::env::var("UPSTREAM_URL") {
        cmd.env("UPSTREAM_URL", url);
    }
    let output = cmd
        .output()
        .await
        .expect("run examples/share-a-tool/proof.sh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "live proof.sh run failed:\n{stdout}"
    );
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(
        !live_mode_enabled(Some("")),
        "empty must disable live mode"
    );
    assert!(
        !live_mode_enabled(Some("0")),
        "MCPHOST_LIVE=0 must disable live mode"
    );
    assert!(
        !live_mode_enabled(Some("true")),
        "only the literal '1' enables live mode"
    );
    assert!(
        live_mode_enabled(Some("1")),
        "MCPHOST_LIVE=1 must enable live mode"
    );
}
