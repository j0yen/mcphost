//! PRD-mcphost-gate-debt-4f1112d, AC5 — a mechanical regression test for
//! the second half of this PRD's own AC5: the two self-referential test
//! names this PRD deleted (see AC4's sibling test's module doc for the
//! five-whys) may never come back under `tests/`. This test fails, naming
//! the offending file, if either does.
//!
//! Deliberately does NOT spell out either banned name anywhere in this
//! file's own source text, including this comment, for the same reason
//! AC4's sibling test doesn't spell out its own banned path (reviewer-
//! agent finding on an earlier draft: a keyword-grep audit of `tests/`
//! would flag a guard file that quotes the very names it forbids as a
//! false positive). Both names are assembled from parts at runtime (see
//! `banned` below) and referred to here only as "the two deleted
//! self-referential test names."
//!
//! The first half of AC5 (`cargo test --quiet` exits 0 three runs in a
//! row) is a process-level claim about the whole suite, not a single
//! file's content — it is verified directly against a fresh flake-audit
//! receipt (three real `cargo test` invocations, all green, deterministic)
//! rather than re-run a third time inside a single `#[test]`, and is
//! recorded as this PRD's own verified-completed evidence for AC5 rather
//! than duplicated here.
//!
//! Same disclosed textual-scan limitation as AC4's sibling test: this
//! catches a name appearing as contiguous text (or as an adjacent
//! concatenation the normalize pass folds back together), not an
//! arbitrarily obfuscated reconstruction of it.

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
            let normalized_text = normalize(&text);
            for needle in banned {
                if text.contains(needle.as_str())
                    || normalized_text.contains(&normalize(needle))
                {
                    offenders.push(format!("{}: {}", path.display(), needle));
                }
            }
        }
    }
}

/// See AC4's sibling test's identical helper for rationale — collapses an
/// adjacent-literal-concatenation obfuscation back to plain text.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '"' | '\'' | '(' | ')' | '+' | '.' | ' ' | '\t' | '\n'))
        .collect()
}
