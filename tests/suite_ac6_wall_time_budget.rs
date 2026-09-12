//! PRD-mcphost-test-suite-consolidation
//! AC6 (P0) -- Given two timed full nextest runs at the same commit, one
//! before and one after the change (or the `gate:` phase from
//! PRD-build-gate-phase-timing when present), When compared in the receipt,
//! Then the after time is <=50% of before.
//!
//! PRD-build-gate-phase-timing has not shipped as of this PRD, so the
//! fallback the requirement names applies: two timed full `cargo nextest
//! run` invocations at this migration commit, isolated `CARGO_TARGET_DIR`,
//! clean caches, recorded in
//! docs/benchmarks/suite-consolidation-wall-time.txt (ac11-load-smoke.txt's
//! convention: the receipt mirrors the number so a claim always cites a
//! file that literally contains it). This test reads that checked-in
//! receipt and asserts the recorded ratio -- it is a regression lock on the
//! receipt staying honest, not a live timing (a live full nextest run is
//! exactly the multi-minute cost this PRD exists to cut, wrong to pay on
//! every `cargo test`).

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Pulls the first "<label>: <number>s" style value out of the receipt text.
fn extract_seconds(text: &str, label: &str) -> f64 {
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix(label) {
            let digits: String = rest
                .trim_start_matches([':', ' '])
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = digits.parse::<f64>() {
                return v;
            }
        }
    }
    panic!("could not find a line starting with {label:?} in the receipt");
}

#[test]
fn recorded_wall_time_is_at_most_half_of_before() {
    let path = repo_root().join("docs/benchmarks/suite-consolidation-wall-time.txt");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    let before = extract_seconds(&text, "before_seconds");
    let after = extract_seconds(&text, "after_seconds");

    assert!(before > 0.0, "recorded before_seconds must be positive, got {before}");
    assert!(
        after <= before * 0.5,
        "recorded after_seconds ({after}) is not <=50% of before_seconds \
         ({before}); the receipt at {} needs a real re-measurement, not a \
         hand-edited number",
        path.display(),
    );
}
