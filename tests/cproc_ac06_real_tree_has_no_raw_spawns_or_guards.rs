//! PRD-mcphost-child-process-test-harness AC6 — at the landing commit the
//! real `tests/` tree passes the lint, holds no hand-rolled child-guard
//! struct, and `busyaudit_ac02` reads its server log through the helper's
//! `stdout_log()`. (The six migrated tests themselves are the rest of the
//! suite: they run, and pass, as ordinary members of it.)

use crate::child_proc_lint;

use child_proc_lint::{repo_root, run_on};
use std::fs;
use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read tests dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_real_tests_tree_passes_the_lint() {
    let (code, out) = run_on(&repo_root().join("tests"));
    assert_eq!(code, 0, "child-proc-lint must pass on the real tree:\n{out}");
    assert!(out.contains("0 offenders"), "{out}");
}

#[test]
fn no_test_declares_its_own_child_guard_struct() {
    // Built at run time so this file does not contain the needle itself.
    let needle = ["struct Child", "Guard"].concat();
    let mut files = Vec::new();
    rust_files(&repo_root().join("tests"), &mut files);
    let hits: Vec<_> = files
        .iter()
        .filter(|p| fs::read_to_string(p).is_ok_and(|s| s.contains(&needle)))
        .collect();
    assert!(hits.is_empty(), "hand-rolled child guards remain: {hits:?}");
}

#[test]
fn busyaudit_ac02_reads_its_server_log_via_stdout_log() {
    let src = fs::read_to_string(
        repo_root().join("tests/busyaudit_ac02_env_override_busy_timeout.rs"),
    )
    .expect("read busyaudit_ac02");
    assert!(src.contains("drain_stdout: true"), "must request the drain");
    assert!(src.contains(".stdout_log()"), "must read the log via stdout_log()");
}
