//! PRD-mcphost-publish-time-tool-lint
//! AC6 (P0) -- Given the 10-08 replay corpus, When lint runs on every file,
//! Then total time <= 5 ms per 10 KB of source and no detector panics on a
//! file with a syntax error (findings `[]`).
//!
//! The corpus under `tests/fixtures/lint_replay/` is the anonymized shape of
//! the 10-08 explore bodies (one file per failure class, plus clean and
//! syntax-error files); each file's first line names the rule it expects.

use mcphost::kinds::python_lint::{lint_checked, lint_source};
use std::path::PathBuf;

fn corpus() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lint_replay");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("lint_replay dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "py"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, String::from_utf8(std::fs::read(&p).expect("read fixture")).expect("utf-8 fixture"))
        })
        .collect();
    files.sort();
    assert!(files.len() >= 6, "corpus has the failure-class, clean and syntax-error files");
    files
}

#[test]
fn total_lint_time_is_within_five_ms_per_ten_kb() {
    let files = corpus();
    let once: usize = files.iter().map(|(_, s)| s.len()).sum();
    // Repeat the corpus up to ~200 KB so the budget (5 ms per 10 KB = 100 ms)
    // is far above timer noise.
    let reps = 200_000 / once + 1;
    let budget_ms = (once * reps * 5).div_ceil(10 * 1024) as u64;
    crate::perf_budget!(budget_ms, {
        for _ in 0..reps {
            for (_, source) in &files {
                std::hint::black_box(lint_source(source, None));
            }
        }
    });
}

#[test]
fn syntax_error_files_never_panic_a_detector_and_find_nothing() {
    let files = corpus();
    for (name, source) in &files {
        let (_, panicked) = lint_checked(source, None);
        assert!(panicked.is_empty(), "{name}: detectors panicked: {panicked:?}");
    }
    let (_, bad) = files.iter().find(|(n, _)| n.starts_with("syntax_error")).expect("syntax_error fixture");
    assert_eq!(lint_source(bad, None), Vec::<serde_json::Value>::new());

    // Every prefix of every fixture (cut on char boundaries) is also a
    // "syntax error": none may panic a detector.
    for (name, source) in &files {
        for (cut, _) in source.char_indices() {
            let (_, panicked) = lint_checked(&source[..cut], None);
            assert!(panicked.is_empty(), "{name}[..{cut}]: detectors panicked: {panicked:?}");
        }
    }
}
