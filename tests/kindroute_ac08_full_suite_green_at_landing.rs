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
//! Unlike a bare string-match on the receipt text, this test also binds
//! the receipt to a `source_hash` over every git-tracked `.rs` file. A
//! receipt that was hand-edited (or simply never regenerated after a
//! later source change, e.g. the kindroute feature being reverted) still
//! contains the right substrings but no longer matches the live tree's
//! hash, so this test fails even though the plain substring checks would
//! pass.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn hash_tracked_rust_sources(root: &PathBuf) -> String {
    let output = Command::new("git")
        .args(["ls-files", "*.rs"])
        .current_dir(root)
        .output()
        .expect("failed to run git ls-files");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut paths: Vec<String> = String::from_utf8(output.stdout)
        .expect("git ls-files output was not utf8")
        .lines()
        .map(str::to_string)
        .collect();
    paths.sort();

    let mut hasher = Sha256::new();
    for path in paths {
        let contents =
            fs::read(root.join(&path)).unwrap_or_else(|e| panic!("read {path}: {e}"));
        hasher.update(path.as_bytes());
        hasher.update(&contents);
    }
    format!("{:x}", hasher.finalize())
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

    let recorded_hash = text
        .lines()
        .find_map(|line| line.strip_prefix("source_hash: "))
        .unwrap_or_else(|| panic!("receipt must record a source_hash line, got: {text}"))
        .trim();
    let actual_hash = hash_tracked_rust_sources(&root);
    assert_eq!(
        recorded_hash, actual_hash,
        "receipt's source_hash does not match the live tree -- the receipt is stale or hand-edited; \
         re-run `wm-build runner exec -- cargo test --workspace` and \
         `cargo clippy --all-targets -- -D warnings` on the runner box, then regenerate \
         docs/benchmarks/kindroute-suite-gate.txt with the new counts and source_hash"
    );
}
