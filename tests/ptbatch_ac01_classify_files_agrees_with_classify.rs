//! contract: batched classify-files gives every test file the verdict classify-file gives it
//! 2026-10-19 (hand PR #449): `ci-test-partition.sh check` and
//! `gen-test-suites.sh` used to run `classify` once per tests/*.rs file (2220
//! subprocesses per call, 58 s per `check` on ryzen7, called by about 15
//! contract tests). Both now use one batched `classify-files` grep. This test
//! holds the batched path to the per-file definition over the full file set,
//! so the partition cannot shift when either one changes.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ptbatch_ac01_classify_files_matches_classify_file_for_every_test_file() {
    let expected = std::fs::read_dir(root().join("tests"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs") && !n.starts_with("suite_"))
        .count();
    assert!(expected > 0);
    let out = Command::new(root().join("scripts/ci-test-partition.sh"))
        .arg("classify-agree")
        .current_dir(root())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "classify-agree failed:\n{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("classify-agree: ok ({expected} files)")),
        "classify-agree must cover all {expected} non-suite tests/*.rs files:\n{stdout}{stderr}"
    );
}
