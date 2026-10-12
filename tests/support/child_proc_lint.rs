//! Scratch-tree runner for the `cproc_ac0N_*` tests of
//! PRD-mcphost-child-process-test-harness: builds a throwaway `tests/` tree
//! (the `flakelint_ac04` scratch convention) holding a copy of the real
//! `support/serve.rs` plus fixture files, and runs `scripts/child-proc-lint.py`
//! over it. Fixture text lives in `tests/fixtures/child_proc_lint/*.tpl`, not
//! in `.rs` files, so the lint never scans its own bad examples; `@GUARD@`
//! is substituted at run time for the same reason.
//!
//! Not a crate module: `#[path = "support/child_proc_lint.rs"] mod
//! child_proc_lint;`.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn script() -> PathBuf {
    repo_root().join("scripts/child-proc-lint.py")
}

/// Fixture text by file name, with `@GUARD@` filled in.
pub fn fixture(name: &str) -> String {
    let path = repo_root().join("tests/fixtures/child_proc_lint").join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
        .replace("@GUARD@", &["Child", "Guard"].concat())
}

/// A scratch dir with `tests/support/serve.rs` copied from the real tree;
/// returns the dir (the `tests/` subdir is what the script scans).
pub fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-cproc-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(dir.join("tests/support")).expect("mkdir scratch");
    fs::copy(
        repo_root().join("tests/support/serve.rs"),
        dir.join("tests/support/serve.rs"),
    )
    .expect("copy serve.rs");
    dir
}

pub fn write(scratch: &Path, name: &str, body: &str) {
    fs::write(scratch.join("tests").join(name), body).expect("write fixture");
}

/// Run the script over `<tests_dir>`; returns (exit code, stdout).
pub fn run_on(tests_dir: &Path) -> (i32, String) {
    let out = Command::new("python3")
        .arg("-I")
        .arg(script())
        .arg(tests_dir)
        .output()
        .expect("run python3 child-proc-lint.py");
    (
        out.status.code().expect("lint exit code"),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

pub fn run(scratch: &Path) -> (i32, String) {
    run_on(&scratch.join("tests"))
}
