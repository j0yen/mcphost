//! PRD-mcphost-contract-suite-generated AC4 -- `scripts/contract-lint.sh tests`
//! exits 1 naming an unmarked file that reads the CHANGELOG, the migrations dir
//! or the suite weights (and the `//! contract: <what it asserts>` line to add),
//! exits 0 once the file carries the marker, and `scripts/flake-lint.sh` runs it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never trips the lint it tests.
// Each body reads a tree-derived source the way the derived lint recognises it
// (PRD-mcphost-contract-suite-markers): an include_str of a marked test's
// artifact, a CARGO_MANIFEST_DIR join of one, and a tree walk of migrations/.
fn triggers() -> Vec<(&'static str, String)> {
    let manifest = "env!(\"CARGO_MANIFEST_DIR\")";
    vec![
        ("changelog", format!("const C: &str = include_str!(\"../{}.md\");\n", "CHANGE".to_string() + "LOG")),
        ("migrations", format!("fn f() {{ std::fs::{}(std::path::Path::new({manifest}).join(\"{}\")); }}\n", "read".to_string() + "_dir", "migra".to_string() + "tions")),
        ("weights", format!("fn f() {{ let _ = std::path::Path::new({manifest}).join(\"scripts/{}\"); }}\n", "ci-suite-".to_string() + "weights.json")),
    ]
}

fn fixture(name: &str, marked: bool, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrs-ac04-{}-{}-{}-{}", std::process::id(), name, marked, SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let marker = if marked { "//! contract: a derived set\n" } else { "//! just a test\n" };
    fs::write(dir.join("zz_probe.rs"), format!("{marker}{body}#[test]\nfn t() {{}}\n")).unwrap();
    dir
}

fn lint(script: &str, dir: &Path) -> std::process::Output {
    Command::new(root().join("scripts").join(script))
        .arg(dir)
        .current_dir(root())
        .output()
        .unwrap()
}

#[test]
fn ctrs_ac04_unmarked_file_fails_naming_file_and_line_to_add() {
    for (name, body) in triggers() {
        let dir = fixture(name, false, &body);
        let out = lint("contract-lint.sh", &dir);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1), "{name}: {stdout}");
        assert!(stdout.contains("zz_probe.rs"), "{name}: {stdout}");
        assert!(stdout.contains("//! contract:"), "{name}: {stdout}");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn ctrs_ac04_marked_file_passes() {
    for (name, body) in triggers() {
        let dir = fixture(name, true, &body);
        let out = lint("contract-lint.sh", &dir);
        assert_eq!(out.status.code(), Some(0), "{name}: {}", String::from_utf8_lossy(&out.stdout));
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn ctrs_ac04_flake_lint_invokes_contract_lint() {
    let script = fs::read_to_string(root().join("scripts/flake-lint.sh")).unwrap();
    assert!(script.contains("contract-lint.sh"));
    let (name, body) = triggers().remove(0);
    let dir = fixture(name, false, &body);
    let out = lint("flake-lint.sh", &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("contract-lint:") && stdout.contains("zz_probe.rs"), "{stdout}");
    let _ = fs::remove_dir_all(&dir);
    let real = lint("flake-lint.sh", &root().join("tests"));
    assert_eq!(real.status.code(), Some(0), "{}", String::from_utf8_lossy(&real.stdout));
}
