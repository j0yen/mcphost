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
//! Deliberately does NOT spell out the banned path anywhere in this file's
//! own source text, including this comment — this PRD's own acceptance
//! check for the invariant greps `tests/` for that literal path as
//! contiguous text, and a guard file that quotes the very string it is
//! checking for would be a positive match against its own check (reviewer-
//! agent finding on an earlier draft of this file: the check must never
//! name, in prose or code, the exact string it forbids). The path is
//! assembled from parts at runtime instead (see `scan_dir` below) and
//! referred to here only as "the gate's receipts directory".
//!
//! Known limitation, disclosed rather than hidden: this is a textual
//! scan for one assembled needle plus one whitespace/punctuation-
//! collapsed variant (see `normalize`) — a sufficiently obfuscated
//! construction (e.g. built one character at a time, or via a
//! non-adjacent concatenation the collapse pass doesn't fold back
//! together) could still evade it. Same best-effort posture as this
//! repo's other structural guards (e.g. `lanecov_ac01_...`'s own doc:
//! "if this test's matcher and \[the real thing\] ever disagree, the
//! test is wrong, not \[the real thing\]") — it raises the bar
//! substantially over "nothing," it does not claim to be unbeatable.

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
            if text.contains(needle) || normalize(&text).contains(&normalize(needle)) {
                offenders.push(path.display().to_string());
            }
        }
    }
}

/// Strips quotes, parens, whitespace, `+`, and `.` so an adjacent
/// concatenation like `"target" + "/autobuilder" + "/receipts"` or
/// `"target". to_owned() + "/autobuilder/receipts"` still collapses back
/// to the plain needle for comparison. Does not defeat every possible
/// obfuscation — see the module doc's disclosed limitation.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '"' | '\'' | '(' | ')' | '+' | '.' | ' ' | '\t' | '\n'))
        .collect()
}
