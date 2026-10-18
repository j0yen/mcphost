//! PRD-mcphost-contract-suite-registry-axis AC4 -- a repo file read through
//! `concat!(env!("CARGO_MANIFEST_DIR"), "/docs/x.md")` counts as a read: an
//! unmarked reader of an artifact a marked test reads that way fails the lint
//! naming `docs/x.md` (no leading `/`), `--fix` prints the follower line, and on
//! the committed tree `--artifacts` still lists the Cited-tree's six paths.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn reader() -> String {
    let manifest = "env!(\"CARGO_MANIFEST_DIR\")";
    format!("fn f() -> String {{ std::fs::read_to_string(concat!({manifest}, \"/docs/x.md\")).unwrap() }}\n")
}

fn fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrx-ac04-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("docs")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("docs/x.md"), "x\n").unwrap();
    let body = reader();
    fs::write(dir.join("tests/zz_definer.rs"), format!("//! contract: a derived doc\n{body}#[test]\nfn t() {{}}\n")).unwrap();
    fs::write(dir.join("tests/zz_reader.rs"), format!("//! just a test\n{body}#[test]\nfn t() {{}}\n")).unwrap();
    dir
}

fn lint(args: &[&str], target: &Path) -> (Option<i32>, String) {
    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .args(args)
        .arg(target)
        .current_dir(root())
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn ctrx_ac04_unmarked_concat_reader_fails_naming_the_path() {
    let dir = fixture();
    let (code, stdout) = lint(&[], &dir.join("tests"));
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("zz_reader.rs") && stdout.contains(" docs/x.md "), "{stdout}");
    assert!(!stdout.contains("/docs/x.md"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac04_fix_prints_the_follower_line() {
    let dir = fixture();
    let (code, stdout) = lint(&["--fix"], &dir.join("tests"));
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.lines().any(|l| l.ends_with("zz_reader.rs: add  //! contract: reads docs/x.md")), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac04_committed_tree_keeps_the_six_cited_paths() {
    let (code, stdout) = lint(&["--artifacts"], &root().join("tests"));
    assert_eq!(code, Some(0), "{stdout}");
    // Escapes keep these names out of this file's own source, so it is not a reader of them.
    for p in [
        "CHANGELO\x47.md",
        "agent/intent-car\x64.json",
        "agent/proof-lane\x73.toml",
        "scripts/ci-suite-weight\x73.json",
        "scripts/ci-test-partition.s\x68",
        "src/scope\x73.rs",
    ] {
        assert!(stdout.lines().any(|l| l == p), "{p} missing from --artifacts:\n{stdout}");
    }
}
