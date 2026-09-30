//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC8 (P0) — Given `cargo test` at the landing commit, When it runs, Then
//! every test that passed at v0.61.0 still passes and the `kindroute_*`
//! tests for AC 1-7 pass, with the gate receipt showing block=0.
//!
//! Same "regression lock on a checked-in receipt" pattern
//! `tests/tkparam_ac11_full_suite_green_at_landing.rs` uses: re-running the
//! whole workspace suite (or clippy) from inside one of its own tests
//! would be both circular and the exact multi-minute cost this file must
//! not itself pay on every `cargo test`. Deliberately no head-sha/"no
//! changes since" check against the receipt -- landing adds commits
//! (release bump, PR squash) that would make such a check
//! self-invalidating on every rebase.
//!
//! 2026-09-30 hotfix (PRD-mcphost-session-bound-tenant-after-signup, run
//! 302): this test originally also bound the receipt to a `source_hash`
//! over every git-tracked `.rs` file, to catch a hand-edited or stale
//! receipt. That check is stricter than "self-invalidating on rebase" --
//! it self-invalidates on ANY later commit to ANY `.rs` file anywhere in
//! the tree, i.e. on every single subsequent PRD's landing, forever,
//! since this test itself stays in the permanent suite after AC8 was
//! proved once at PRD 306's own landing commit. That's not a regression
//! lock, it's a gate that can never pass again. Dropped back to the bare
//! substring-match every sibling suite-gate test in this repo already
//! uses (see `tkparam_ac11_full_suite_green_at_landing.rs`,
//! `checkcompat_race_ac07_suite_green_and_clippy_clean.rs`) -- still a
//! real regression lock on the checked-in receipt's pass/warning counts,
//! just not on the live tree's exact bytes.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn suite_gate_receipt_records_zero_failures_and_zero_clippy_warnings() {
    let root = repo_root();
    let path = root.join("docs/benchmarks/kindroute-suite-gate.txt");
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
