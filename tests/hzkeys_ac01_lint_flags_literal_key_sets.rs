//! PRD-mcphost-healthz-anon-keys-const AC1 (P0) -- Given a scratch tests tree
//! with one file asserting the anonymous key set as an array and another
//! spelling it as a `json!` object, When `scripts/healthz-keys-lint.py` runs,
//! Then exit 1 naming both file:lines; given both rewritten to
//! `common::anon_healthz_keys()`, Then exit 0.
//!
//! Fixtures are generated from the crate const, never spelled here, so this
//! file is itself clean under the lint it tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use mcphost::ANON_HEALTHZ_KEYS;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `<scratch>/scripts/healthz-keys-lint.py` + `<scratch>/src/build_info.rs`
/// (the script resolves its root from its own location) + `<scratch>/tests/`.
fn scratch(tag: &str, files: &[(&str, String)]) -> PathBuf {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hzkeys-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
    ));
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::copy(root.join("scripts/healthz-keys-lint.py"), dir.join("scripts/healthz-keys-lint.py")).unwrap();
    fs::copy(root.join("src/build_info.rs"), dir.join("src/build_info.rs")).unwrap();
    for (name, body) in files {
        fs::write(dir.join("tests").join(name), body).unwrap();
    }
    dir
}

fn run_lint(dir: &Path) -> std::process::Output {
    Command::new("python3")
        .arg("scripts/healthz-keys-lint.py")
        .arg("tests")
        .current_dir(dir)
        .output()
        .expect("run python3 healthz-keys-lint.py")
}

fn array_fixture() -> String {
    format!("fn t() {{\n    assert_eq!(keys, {ANON_HEALTHZ_KEYS:?});\n}}\n")
}

fn json_fixture() -> String {
    let fields: Vec<String> = ANON_HEALTHZ_KEYS.iter().rev().map(|k| format!("\"{k}\": v")).collect();
    format!("fn t() {{\n    let a = 1;\n    assert_eq!(anon, json!({{{}}}));\n}}\n", fields.join(", "))
}

#[test]
fn literal_key_sets_fail_naming_both_file_lines_and_the_rewrite_passes() {
    let dir = scratch(
        "ac01",
        &[("arr.rs", array_fixture()), ("obj.rs", json_fixture())],
    );
    let out = run_lint(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "literal key sets must exit 1; stdout:\n{stdout}");
    assert!(stdout.contains("tests/arr.rs:2"), "must name tests/arr.rs:2; stdout:\n{stdout}");
    assert!(stdout.contains("tests/obj.rs:3"), "must name tests/obj.rs:3; stdout:\n{stdout}");

    let rewritten = "fn t() {\n    assert_eq!(keys, common::anon_healthz_keys());\n    let _ = common::anon_healthz_keys();\n}\n";
    for name in ["arr.rs", "obj.rs"] {
        fs::write(dir.join("tests").join(name), rewritten).unwrap();
    }
    let out = run_lint(&dir);
    assert_eq!(
        out.status.code(),
        Some(0),
        "rewritten files must exit 0; stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn value_reads_and_tests_common_are_not_findings() {
    let value_reads = "fn t() {\n    assert_eq!(b[\"ok\"], 1);\n    assert!(b[\"version\"].is_string());\n    assert!(b[\"git_sha\"].is_null());\n}\n";
    let dir = scratch("ac01-values", &[("values.rs", value_reads.to_string())]);
    fs::create_dir_all(dir.join("tests/common")).unwrap();
    fs::write(dir.join("tests/common/mod.rs"), array_fixture()).unwrap();
    let out = run_lint(&dir);
    assert_eq!(
        out.status.code(),
        Some(0),
        "per-key value reads and tests/common are exempt; stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    fs::remove_dir_all(&dir).ok();
}
