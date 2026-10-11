//! PRD-mcphost-healthz-anon-keys-const AC4 (P0) -- the lint script's
//! detection set is parsed from `ANON_HEALTHZ_KEYS` in `src/build_info.rs`,
//! not spelled in the script: the script has no key-name literal, and
//! changing the const (in a scratch copy) changes what it flags.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn script_spells_none_of_the_key_names() {
    let script = fs::read_to_string(root().join("scripts/healthz-keys-lint.py")).unwrap();
    for name in mcphost::ANON_HEALTHZ_KEYS {
        assert!(!script.contains(&format!("\"{name}\"")), "script spells \"{name}\"; it must parse the const");
    }
}

#[test]
fn changing_the_const_changes_the_detection_set() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hzkeys-ac04-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::copy(root().join("scripts/healthz-keys-lint.py"), dir.join("scripts/healthz-keys-lint.py")).unwrap();

    // A fixture spelling a set of names that is NOT the real const.
    let names = ["alpha_k", "beta_k"];
    fs::write(dir.join("tests/lit.rs"), format!("fn t() {{\n    assert_eq!(keys, {names:?});\n}}\n")).unwrap();
    let lint = |dir: &PathBuf| {
        Command::new("python3").arg("scripts/healthz-keys-lint.py").arg("tests").current_dir(dir).output().unwrap()
    };

    let src = fs::read_to_string(root().join("src/build_info.rs")).unwrap();
    fs::write(dir.join("src/build_info.rs"), &src).unwrap();
    assert_eq!(lint(&dir).status.code(), Some(0), "names outside the const are not flagged");

    let patched = src.replace(
        &format!("ANON_HEALTHZ_KEYS: &[&str] = &{:?};", mcphost::ANON_HEALTHZ_KEYS),
        &format!("ANON_HEALTHZ_KEYS: &[&str] = &{names:?};"),
    );
    assert_ne!(patched, src, "const line not found to patch");
    fs::write(dir.join("src/build_info.rs"), patched).unwrap();
    let out = lint(&dir);
    assert_eq!(out.status.code(), Some(1), "after the const changes, the new set is flagged");
    assert!(String::from_utf8_lossy(&out.stdout).contains("tests/lit.rs:2"));
    fs::remove_dir_all(&dir).ok();
}
