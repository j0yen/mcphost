//! PRD-mcphost-contract-suite-registry-axis AC2 -- on a fixture tree whose
//! `src/handler.rs` declares a registry fn, a marked test that defines it and an
//! unmarked test that calls it, `scripts/contract-lint.sh` exits 1 naming the
//! unmarked file, the registry and the definer; `--fix` prints the line to add;
//! an unmarked file naming an undeclared `pub fn` exits 0.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never trips the lint it tests.
fn reg() -> String {
    format!("host_tool_{}", "descriptors")
}

fn fixture(unmarked_body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrx-ac02-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(
        dir.join("src/handler.rs"),
        format!("pub fn {}() -> Vec<u8> {{ vec![] }}\npub fn other_fn() {{}}\n", reg()),
    )
    .unwrap();
    fs::write(
        dir.join("tests/zz_definer.rs"),
        format!("//! contract: registry handler::{} \u{2014} every verb forwards\n#[test]\nfn t() {{ let _ = {}(); }}\n", reg(), reg()),
    )
    .unwrap();
    fs::write(dir.join("tests/zz_walker.rs"), format!("//! just a test\n{unmarked_body}#[test]\nfn t() {{}}\n")).unwrap();
    dir
}

fn lint(args: &[&str], dir: &Path) -> (Option<i32>, String) {
    let out = Command::new(root().join("scripts/contract-lint.sh"))
        .args(args)
        .arg(dir.join("tests"))
        .current_dir(root())
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn ctrx_ac02_unmarked_walker_fails_naming_file_registry_and_definer() {
    let dir = fixture(&format!("fn f() {{ let _ = {}(); }}\n", reg()));
    let (code, stdout) = lint(&[], &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("zz_walker.rs"), "{stdout}");
    assert!(stdout.contains(&format!("handler::{}", reg())), "{stdout}");
    assert!(stdout.contains("zz_definer.rs"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac02_fix_prints_the_follower_line() {
    let dir = fixture(&format!("fn f() {{ let _ = {}(); }}\n", reg()));
    let (code, stdout) = lint(&["--fix"], &dir);
    assert_eq!(code, Some(1), "{stdout}");
    let want = format!("zz_walker.rs: add  //! contract: reads registry handler::{}", reg());
    assert!(stdout.lines().any(|l| l.ends_with(&want)), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac02_undeclared_pub_fn_is_not_a_registry() {
    let dir = fixture("fn f() { let _ = other_fn(); }\n");
    let (code, stdout) = lint(&[], &dir);
    assert_eq!(code, Some(0), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}
