//! PRD-mcphost-gate-debt-4f1112d, AC5 — a mechanical regression test for
//! the second half of this PRD's own AC5: no test named
//! `flake_audit_does_not_block` or `cold_build_time_does_not_block` may
//! exist under `tests/`. Both were the self-referential fixtures this
//! PRD deleted (see AC4's sibling test's module doc for the five-whys);
//! this test fails, naming the offending file, if either name ever comes
//! back.
//!
//! The first half of AC5 (`cargo test --quiet` exits 0 three runs in a
//! row) is a process-level claim about the whole suite, not a single
//! file's content — it is verified directly against a fresh
//! `flake-audit-receipt.json` (three real `cargo test` invocations,
//! `deterministic: true`, `exit_codes: [0, 0, 0]`) rather than re-run a
//! third time inside a single `#[test]`, and is recorded as this PRD's
//! own verified-completed evidence for AC5 rather than duplicated here.
//!
//! Same self-match avoidance as AC4's sibling test: the two banned names
//! are assembled from parts at runtime so this file's own source is never
//! a positive match for its own check.

use std::fs;
use std::path::Path;

#[test]
fn banned_flake_audit_selftest_names_do_not_exist() {
    let banned = [
        ["flake_audit", "does_not_block"].join("_"),
        ["cold_build_time", "does_not_block"].join("_"),
    ];

    let self_file = Path::new(file!())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    scan_dir(&tests_dir, &banned, &self_file, &mut offenders);

    assert!(
        offenders.is_empty(),
        "banned flake-audit self-referential test name(s) found (AC5 regression): {offenders:?}"
    );
}

fn scan_dir(dir: &Path, banned: &[String], self_file: &str, offenders: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, banned, self_file, offenders);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == self_file {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            for needle in banned {
                if text.contains(needle.as_str()) {
                    offenders.push(format!("{}: {}", path.display(), needle));
                }
            }
        }
    }
}
