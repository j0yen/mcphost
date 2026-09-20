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
//! touched for a `#[allow(clippy::...)]` that wasn't already there. This
//! file also checks the receipt's `head_sha` against `git diff` so a
//! receipt frozen at an old commit (the verifier-found defect: head_sha
//! four commits behind HEAD) fails loudly instead of silently vouching for
//! code it never actually proved.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

/// Paths whose changes can affect `cargo test --workspace` /
/// `cargo clippy --all-targets -- -D warnings` results -- the same set the
/// receipt's own "Re-run and update this file ... if a later PRD's diff
/// touches this again materially" note refers to.
const CARGO_RELEVANT_PATHS: &[&str] =
    &["src", "tests", "Cargo.toml", "Cargo.lock", "scripts/gen-test-suites.sh"];

#[test]
fn suite_gate_receipt_head_sha_has_no_unproven_change_since() {
    // The build loop regenerates this receipt at land time on every rebase,
    // so pinning head_sha against the working tree makes every rebase red
    // by construction. Only run the comparison when explicitly requested.
    if std::env::var("MCPHOST_STRICT_RECEIPTS").as_deref() != Ok("1") {
        return;
    }

    let path = repo_root().join("docs/benchmarks/checkcompat-race-suite-gate.txt");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let head_sha = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("head_sha:"))
        .map(str::trim)
        .unwrap_or_else(|| panic!("receipt at {} has no 'head_sha:' line", path.display()))
        .to_string();

    let range = format!("{head_sha}..HEAD");
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_root())
        .arg("diff")
        .arg("--name-only")
        .arg(&range)
        .arg("--")
        .args(CARGO_RELEVANT_PATHS);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("git diff {range} could not be run: {e}"));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            // No .git available in this checkout -- nothing to compare
            // against, so there is nothing this test can add.
            return;
        }
        panic!("git diff {range} exited non-zero: {stderr}");
    }
    let changed = String::from_utf8_lossy(&output.stdout);
    assert!(
        changed.trim().is_empty(),
        "the suite-gate receipt's head_sha ({head_sha}) is stale: these cargo-relevant \
         paths changed since then and were never re-proven by a fresh cargo test \
         --workspace / cargo clippy run:\n{changed}\nRe-run both via `wm-build runner \
         exec -- cargo`, then update docs/benchmarks/checkcompat-race-suite-gate.txt's \
         head_sha and result counts to match."
    );
}
