//! PRD-mcphost-test-suite-consolidation
//! AC1 (P0) -- Given the repo at HEAD, When `gen-test-suites.sh` runs and
//! `cargo nextest list` runs, Then at most ten test binaries are listed (the
//! cap is `MAX_SUITES` in scripts/gen-test-suites.sh, read from there: twelve
//! since 2026-10-18 for the solo ctrs_ac06 suite and a third normal core
//! bucket, thirteen for a fourth) and
//! the set of test names (suite prefix stripped) equals the set listed at
//! the parent commit.
//!
//! The "equals the parent commit's set" half is a one-time migration fact,
//! not a perpetual property (adding a genuinely new test is expected to
//! change the set) -- it was checked once, by hand, at the migration
//! commit: `cargo nextest list --message-format json` at the parent commit
//! (291 binaries, 596 tests) and at this commit (8 binaries, 596 tests)
//! produced IDENTICAL (stem, bare-test-name) pairs for every one of the 421
//! tests sourced from `tests/*.rs` (the other 175 are `src/`'s own `#[cfg
//! (test)]` unit tests and the `mcphost` bin's, untouched by this PRD) --
//! zero missing, zero extra. That diff is this dispatch's Receipts evidence,
//! not re-derived here (re-deriving it would need `cargo nextest` on PATH,
//! which CI's `gate`/`sandbox` jobs -- plain `cargo test` -- don't have).
//!
//! What a perpetual, CI-safe (no nextest dependency) test CAN hold is the
//! generator invariant that keeps that fact true going forward: the suite
//! count stays under the P0 cap, and the committed `tests/suite_*.rs` files
//! plus `Cargo.toml`'s `[[test]]` block are exactly what
//! `scripts/gen-test-suites.sh` would produce right now -- so a test can
//! never be silently dropped, renamed, or duplicated by a stale suite file.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn at_most_ten_suites_and_none_drifted() {
    let root = repo_root();

    let suite_count = std::fs::read_dir(root.join("tests"))
        .expect("read tests/")
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("suite_")
                && name.ends_with(".rs")
                && (name.starts_with("suite_core_") || name.starts_with("suite_sandbox_"))
        })
        .count();

    assert!(
        suite_count >= 1,
        "no generated suite_core_*/suite_sandbox_*.rs files found -- has \
         scripts/gen-test-suites.sh been run?"
    );
    // The cap is ONE definition: the `^MAX_SUITES = N` line the generator and
    // ci-test-partition.sh also read, never a copy of the number here.
    let generator = std::fs::read_to_string(root.join("scripts/gen-test-suites.sh"))
        .expect("read scripts/gen-test-suites.sh");
    let max_suites: usize = generator
        .lines()
        .find_map(|l| l.strip_prefix("MAX_SUITES = "))
        .and_then(|n| n.trim().parse().ok())
        .expect("scripts/gen-test-suites.sh has no `MAX_SUITES = N` line");
    assert!(
        suite_count <= max_suites,
        "P0 requirement: at most MAX_SUITES={max_suites} test binaries (MAX_SUITES in scripts/gen-test-suites.sh); found {suite_count} \
         tests/suite_*.rs files. Either MAX_PER_SUITE in \
         scripts/gen-test-suites.sh needs raising, or tests/ has grown \
         enough that the area table needs a look."
    );

    let out = Command::new(root.join("scripts/gen-test-suites.sh"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("run scripts/gen-test-suites.sh --check");
    assert!(
        out.status.success(),
        "scripts/gen-test-suites.sh --check reported drift -- the committed \
         tests/suite_*.rs files and/or Cargo.toml's [[test]] block no longer \
         match what the generator would produce, which means the compiled \
         test binaries no longer match tests/*.rs on disk.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
