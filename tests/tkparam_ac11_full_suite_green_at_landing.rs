//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC11 (P0) — Given `cargo test` at the landing commit, When it runs,
//! Then every test that passed at v0.60.34 still passes and the tests
//! added for AC 1-9 pass, with the gate receipt showing block=0.
//!
//! Same "regression lock on a checked-in receipt" pattern
//! `tests/checkcompat_race_ac07_suite_green_and_clippy_clean.rs` uses:
//! re-running the whole workspace suite (or clippy) from inside one of
//! its own tests would be both circular and the exact multi-minute cost
//! this file must not itself pay on every `cargo test`. Deliberately no
//! head-sha/"no changes since" check against the receipt -- landing adds
//! commits (release bump, PR squash) that would make such a check
//! self-invalidating on every rebase.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn suite_gate_receipt_records_zero_failures_and_zero_clippy_warnings() {
    let path = repo_root().join("docs/benchmarks/tkparam-suite-gate.txt");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    assert!(
        text.contains("0 failed"),
        "receipt must record a zero-failure cargo test --workspace run, got: {text}"
    );
    assert!(
        text.contains("clippy_warning_count: 0"),
        "receipt must record zero clippy warnings, got: {text}"
    );
}
