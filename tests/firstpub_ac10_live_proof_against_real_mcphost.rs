//! PRD-mcphost-first-publish-real-kind
//! AC10 (P0, Live; deferred -- proving it needs a real deploy to
//! production `https://mcphost.dev`, which this coding sandbox cannot
//! perform: no provisioning/deploy access, and the AC's own evidence
//! requirement is a receipt under docs/receipts/ naming mcphost-1's
//! healthz version after a real deploy). Same shape as
//! `tests/mcphost_database_in_a_minute_ac08_live_test_skipped_without_env.rs`
//! and its siblings: the always-on test below proves the gating mechanism
//! deterministically; the real leg only runs with `MCPHOST_LIVE=1` set,
//! which never happens in a normal `cargo test`.

use tokio::process::Command;

/// Pure predicate, deliberately not reading `std::env` itself, so the
/// gating logic is asserted deterministically instead of depending on (and
/// possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// AC10 (P0, Live) -- runs `examples/first-run/proof.sh` against the real
/// `$MCPHOST_URL` (default `https://mcphost.dev/mcp`) end to end: signs up,
/// publishes `text_stats` as a python tool, calls it, and asserts the real
/// output. Only executes with `MCPHOST_LIVE=1`; otherwise this is a
/// successful no-op.
#[tokio::test]
async fn live_proof_against_real_mcphost_when_enabled() {
    if !live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
        eprintln!(
            "skipping live proof.sh run: set MCPHOST_LIVE=1 (and optionally MCPHOST_URL) \
             to run it against a real endpoint"
        );
        return;
    }
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/first-run/proof.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    if let Ok(url) = std::env::var("MCPHOST_URL") {
        cmd.env("MCPHOST_URL", url);
    }
    let output = cmd
        .output()
        .await
        .expect("run examples/first-run/proof.sh");
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
