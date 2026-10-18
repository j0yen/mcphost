//! PRD-mcphost-contract-suite-registry-axis AC3 -- a marker naming a registry
//! that `src/` does not define fails the lint with `stale marker` and the symbol;
//! a follower line with no definer fails with `no definer for registry <sym>`;
//! definer plus follower exits 0 and `--artifacts` prints the registry once.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never trips the lint it tests.
fn wv() -> String {
    format!("write_verbs::{}", "WRITE_".to_string() + "VERBS")
}

fn fixture(files: &[(&str, String)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrx-ac03-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/handler.rs"), "pub fn present_fn() {}\n").unwrap();
    fs::write(dir.join("src/write_verbs.rs"), "pub const WRITE_VERBS: &[&str] = &[];\n").unwrap();
    for (name, body) in files {
        fs::write(dir.join("tests").join(name), format!("{body}#[test]\nfn t() {{}}\n")).unwrap();
    }
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
fn ctrx_ac03_unresolvable_registry_is_a_stale_marker() {
    let dir = fixture(&[("zz_stale.rs", "//! contract: registry handler::no_such_fn \u{2014} gone\n".into())]);
    let (code, stdout) = lint(&[], &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("stale marker") && stdout.contains("no_such_fn"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac03_follower_without_definer_fails() {
    let dir = fixture(&[("zz_follower.rs", format!("//! contract: reads registry {}\n", wv()))]);
    let (code, stdout) = lint(&[], &dir);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains(&format!("no definer for registry {}", wv())), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrx_ac03_definer_and_follower_pass_and_artifacts_lists_registry_once() {
    let dir = fixture(&[
        ("zz_definer.rs", format!("//! contract: registry {} \u{2014} every write verb\n", wv())),
        ("zz_follower.rs", format!("//! contract: reads registry {}\n", wv())),
    ]);
    let (code, stdout) = lint(&[], &dir);
    assert_eq!(code, Some(0), "{stdout}");
    let (code, stdout) = lint(&["--artifacts"], &dir);
    assert_eq!(code, Some(0), "{stdout}");
    let line = format!("registry {}", wv());
    assert_eq!(stdout.lines().filter(|l| *l == line).count(), 1, "{stdout}");
    let _ = fs::remove_dir_all(&dir);
}
