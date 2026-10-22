//! PRD-mcphost-test-suite-flake-lints (request-log capture flake class)
//! AC1 -- Given any file under `tests/`, When it installs a scoped tracing
//! dispatcher (`set_default`, `with_default`) or recomputes the process-wide
//! callsite interest cache (`rebuild_interest_cache`), Then this test fails
//! and names the file, line and the helper to use instead.
//!
//! Those three calls are the flake class behind `sessbind_ac10`,
//! `incbanb_ac03` and `gsigb_ac03` (CI run 39681889523, shard 5/5): in a
//! consolidated suite binary a scoped dispatcher swap races other threads'
//! callsite registration, `tracing` caches the callsite as "never
//! interested", and the event is dropped (missing, not late). Capture
//! through `tests/common/tracecap.rs` instead: one global subscriber
//! installed once per binary, events routed per emitting thread.
//!
//! `set_global_default` is a different pattern (a singleton suite binary,
//! `scripts/gen-test-suites.sh`) and is not flagged. Comment lines are not
//! scanned; the helper itself is the only exempt file. The needles are
//! assembled at run time so this file does not trip its own scan.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::tracecap;

fn needles() -> [String; 3] {
    [
        ["set_", "default("].concat(),
        ["with_", "default("].concat(),
        ["rebuild_", "interest_cache("].concat(),
    ]
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `path:line: text` for every non-comment line under `tests_dir` that
/// contains a needle, except in the helper `common/tracecap.rs`.
fn violations(tests_dir: &Path) -> Vec<String> {
    let needles = needles();
    let mut files = Vec::new();
    rs_files(tests_dir, &mut files);
    files.sort();
    let mut found = Vec::new();
    for file in files {
        let rel = file.strip_prefix(tests_dir).unwrap_or(&file);
        if rel == Path::new("common/tracecap.rs") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if needles.iter().any(|n| line.contains(n.as_str())) {
                found.push(format!("tests/{}:{}: {}", rel.display(), i + 1, line.trim()));
            }
        }
    }
    found
}

#[test]
fn tracecap_ac01_no_test_file_installs_a_scoped_tracing_default() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let found = violations(&tests_dir);
    assert!(
        found.is_empty(),
        "bare scoped-subscriber capture in a shared test binary drops events \
         (callsite-interest race); use tests/common/tracecap.rs \
         (`common::tracecap::capture_json()` / `capture_text()`):\n{}",
        found.join("\n")
    );
}

#[test]
fn tracecap_ac01_scan_flags_each_needle_and_skips_comments_and_the_helper() {
    // A scratch tree this test creates and removes itself.
    let root = std::env::temp_dir().join(format!(
        "mcphost-tracecap-guard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let tests_dir = root.join("tests");
    fs::create_dir_all(tests_dir.join("common")).expect("mkdir scratch");
    let [set, with, rebuild] = needles();
    fs::write(tests_dir.join("a_set.rs"), format!("fn t() {{ let _g = tracing::subscriber::{set}x); }}\n")).unwrap();
    fs::write(tests_dir.join("b_with.rs"), format!("fn t() {{ tracing::subscriber::{with}x, || ()); }}\n")).unwrap();
    fs::write(tests_dir.join("c_rebuild.rs"), format!("fn t() {{ tracing::callsite::{rebuild}); }}\n")).unwrap();
    fs::write(tests_dir.join("d_comment.rs"), format!("// mentions {set}x) only in a comment\n")).unwrap();
    fs::write(tests_dir.join("e_global.rs"), "fn t() { subscriber::set_global_default(x); }\n").unwrap();
    fs::write(tests_dir.join("common/tracecap.rs"), format!("fn t() {{ {set}x); {rebuild}); }}\n")).unwrap();

    let found = violations(&tests_dir);
    fs::remove_dir_all(&root).expect("remove scratch");

    assert_eq!(found.len(), 3, "expected exactly the three bare files: {found:?}");
    assert!(found[0].starts_with("tests/a_set.rs:1:"), "{found:?}");
    assert!(found[1].starts_with("tests/b_with.rs:1:"), "{found:?}");
    assert!(found[2].starts_with("tests/c_rebuild.rs:1:"), "{found:?}");
}

/// One callsite, so both calls below register and hit the same `Interest`.
fn emit_probe_line() {
    tracing::info!("tracecap-callsite-probe");
}

#[test]
fn tracecap_ac01_callsite_first_hit_by_an_uncapturing_thread_is_still_captured() {
    // Installs the global subscriber and opens a capture on THIS thread only.
    let capture = tracecap::capture_text();
    // The callsite is registered for the first time by a thread with no
    // capture open. A static `filter_fn` caches "never" here and drops the
    // line below (CI run 39692988549, `support_ac02`).
    std::thread::spawn(emit_probe_line).join().expect("probe thread");
    emit_probe_line();
    let text = capture.finish();
    let hits = text.lines().filter(|l| l.contains("tracecap-callsite-probe")).count();
    assert_eq!(hits, 1, "expected exactly the capturing thread's probe line, got:\n{text}");
}
