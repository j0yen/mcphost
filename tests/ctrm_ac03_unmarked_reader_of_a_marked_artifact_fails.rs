//! PRD-mcphost-contract-suite-markers AC3 -- given a marked test reading an
//! artifact through a `CARGO_MANIFEST_DIR` join and an unmarked test reading the
//! same path, `scripts/contract-lint.sh <dir>` exits 1 naming the unmarked file,
//! the artifact and the marked file that defines it; an unmarked file that reads
//! only a `tests/fixtures/` path exits 0.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never names the artifact.
fn artifact() -> String {
    format!("{}/{}", "agent", "intent-card.json")
}

fn reader(path_chain: &str) -> String {
    format!("fn f() {{ let _ = std::path::Path::new(env!(\"CARGO_MANIFEST_DIR\")){path_chain}; }}\n#[test]\nfn t() {{}}\n")
}

fn chain(path: &str) -> String {
    path.split('/').map(|p| format!(".join(\"{p}\")")).collect::<String>()
}

fn fixture(unmarked_path: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrm-ac03-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("aa_marked.rs"), format!("//! contract: a derived file\n{}", reader(&chain(&artifact())))).unwrap();
    fs::write(dir.join("zz_unmarked.rs"), format!("//! just a test\n{}", reader(&chain(unmarked_path)))).unwrap();
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
fn ctrm_ac03_unmarked_reader_fails_naming_file_artifact_and_definer() {
    let dir = fixture(&artifact());
    let (rc, out) = lint(&dir);
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("zz_unmarked.rs"), "{out}");
    assert!(out.contains(&artifact()), "{out}");
    assert!(out.contains("aa_marked.rs"), "{out}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ctrm_ac03_fixture_only_reader_passes() {
    let dir = fixture("tests/fixtures/intent-card.json");
    let (rc, out) = lint(&dir);
    assert_eq!(rc, 0, "{out}");
    let _ = fs::remove_dir_all(&dir);
}
