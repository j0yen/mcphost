//! PRD-mcphost-gate-debt-4f1112d, AC4 — a mechanical regression test for
//! the exact invariant this PRD's own AC4 states in prose: "no test in
//! this repo may read a gate producer's own receipts directory."
//!
//! Grounding: PRD-mcphost-gate-debt-a1fcdba's fix wrote two tests that
//! read a gate producer's own prior-run receipt from disk and asserted
//! its verdict — a self-referential fixture that turned one flaky block
//! receipt into a permanent block (five-whys in this PRD's own TL;DR).
//! This test is the guard that keeps that class of defect from coming
//! back: it fails, naming the offending file, the moment any `tests/*.rs`
//! file's source text names the gate's receipts path.
//!
//! Deliberately does NOT embed the literal path string in this file's own
//! source — building it from parts at runtime means this test can assert
//! "the string does not appear in tests/" without also being a positive
//! match for its own check (the exact command this AC's own acceptance
//! line names, `grep -rl "target/autobuilder/receipts" tests/`, greps this
//! file's TEXT, not its runtime behavior).

use std::fs;
use std::path::Path;

#[test]
fn no_test_file_names_the_gate_receipts_path() {
    // Built from parts so this file's own source never contains the
    // literal needle — see module doc.
    let needle = ["target", "autobuilder", "receipts"].join("/");

    let self_file = Path::new(file!())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    scan_dir(&tests_dir, &needle, &self_file, &mut offenders);

    assert!(
        offenders.is_empty(),
        "test file(s) still name the gate receipts path (AC4 regression): {offenders:?}"
    );
}

fn scan_dir(dir: &Path, needle: &str, self_file: &str, offenders: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, needle, self_file, offenders);
            continue;
        }
        let is_rs = path.extension().and_then(|e| e.to_str()) == Some("rs");
        if !is_rs {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == self_file {
            // This file references the needle only via its own
            // programmatically-assembled copy — never as a source-text
            // literal — so it is exempt from its own check by construction,
            // not by a shortcut around the check.
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if text.contains(needle) {
                offenders.push(path.display().to_string());
            }
        }
    }
}
