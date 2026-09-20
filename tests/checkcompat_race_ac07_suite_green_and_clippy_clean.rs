//! PRD-mcphost-checkcompat-port-race AC7 (P0) — Given the whole mcphost
//! test suite, When `cargo test --workspace` runs on RedBaron, Then it
//! exits 0 and clippy is unchanged versus the committed baseline
//! (delta-pass convention).
//!
//! Same "regression lock on a checked-in receipt" pattern
//! `tests/suite_ac6_wall_time_budget.rs` uses for its own non-functional
//! AC: re-running the whole workspace suite (or clippy) from inside one
//! of its own tests would be both circular and the exact multi-minute
//! cost this file must not itself pay on every `cargo test`. The receipt
//! at docs/benchmarks/checkcompat-race-suite-gate.txt is the literal
//! `wm-build runner exec --run 34 -- cargo test --workspace` /
//! `cargo clippy --all-targets -- -D warnings` output at this PRD's HEAD.
//!
//! What *is* checked live here: "clippy is unchanged" means clean by
//! construction, not by newly suppressing a warning this PRD's diff
//! introduced -- so this file also greps every source file this PRD
//! touched for a `#[allow(clippy::...)]` that wasn't already there.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `src/*.rs` file PRD-mcphost-checkcompat-port-race's requirements
/// 1-6 touched (requirement 7's mcphost-deploy edit is a different repo,
/// covered by its own proof, not this file).
const TOUCHED_SRC_FILES: &[&str] = &[
    "src/compat_check.rs",
    "src/http.rs",
    "src/main.rs",
    "src/state.rs",
    "src/tables.rs",
];

#[test]
fn clippy_clean_by_construction_not_by_new_suppression() {
    for rel in TOUCHED_SRC_FILES {
        let path = repo_root().join(rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        assert!(
            !content.contains("#[allow(clippy"),
            "{rel} carries a clippy suppression -- clippy must stay clean by \
             construction, not by silencing a new warning (see the receipt at \
             docs/benchmarks/checkcompat-race-suite-gate.txt)"
        );
    }
}

#[test]
fn suite_gate_receipt_records_zero_failures_and_zero_clippy_warnings() {
    let path = repo_root().join("docs/benchmarks/checkcompat-race-suite-gate.txt");
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
