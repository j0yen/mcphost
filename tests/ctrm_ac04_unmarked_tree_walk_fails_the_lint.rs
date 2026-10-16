//! PRD-mcphost-contract-suite-markers AC4 -- an unmarked test that walks a
//! `CARGO_MANIFEST_DIR`-joined tree outside tests/ with a directory listing
//! fails `scripts/contract-lint.sh <dir>` naming the file and the tree-walk
//! rule; the same file with a `//! contract:` line passes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never matches the rule it tests.
fn walker() -> String {
    let listing = "read".to_string() + "_dir";
    let manifest = "env!(\"CARGO_MANIFEST_DIR\")";
    format!("fn f() {{ let _ = std::fs::{listing}(std::path::Path::new({manifest}).join(\"src\")); }}\n#[test]\nfn t() {{}}\n")
}

fn fixture(marked: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrm-ac04-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let head = if marked { "//! contract: the src tree\n" } else { "//! just a test\n" };
    fs::write(dir.join("zz_walker.rs"), format!("{head}{}", walker())).unwrap();
    dir
}

fn lint(dir: &Path) -> (i32, String) {
    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .arg(dir)
        .current_dir(root())
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn ctrm_ac04_unmarked_walker_fails_naming_file_and_rule() {
    let dir = fixture(false);
    let (rc, out) = lint(&dir);
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("zz_walker.rs"), "{out}");
    assert!(out.contains("tree-walk"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrm_ac04_marked_walker_passes() {
    let dir = fixture(true);
    let (rc, out) = lint(&dir);
    assert_eq!(rc, 0, "{out}");
    let _ = fs::remove_dir_all(&dir);
}
