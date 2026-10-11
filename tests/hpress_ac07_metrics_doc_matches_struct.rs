//! PRD-mcphost-status-host-pressure AC7 (P1): `docs/metrics.md`'s host-field
//! table matches `HostPressure`'s field list, and the doc check fails when a
//! field is renamed. Runs the real script against a scratch copy of the files
//! it touches (same convention as `bridgedisc_ac07_sandbox_api_doc_check`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const TOUCHED: &[&str] = &[
    "scripts/host-pressure-doc-check.sh",
    "src/hostpressure.rs",
    "docs/metrics.md",
];

fn scratch_copy(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hpress-ac07-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    for rel in TOUCHED {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(repo_root().join(rel), &dest).unwrap_or_else(|e| panic!("copy {rel}: {e}"));
    }
    dir
}

fn run_check(dir: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(dir.join("scripts/host-pressure-doc-check.sh"))
        .env_remove("HOST_PRESSURE_SOURCE")
        .env_remove("HOST_PRESSURE_DOC")
        .output()
        .expect("run host-pressure-doc-check.sh")
}

#[test]
fn committed_table_matches_the_struct_field_list() {
    let dir = scratch_copy("clean");
    let out = run_check(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout} {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("fields=7 docs_in_sync=1"), "{stdout}");

    // The table names exactly the serialized keys of a live sample.
    let doc = std::fs::read_to_string(repo_root().join("docs/metrics.md")).unwrap();
    let sample = serde_json::to_value(mcphost::hostpressure::sample(Path::new("/nonexistent"))).unwrap();
    for key in sample.as_object().unwrap().keys() {
        assert!(doc.contains(&format!("| `{key}` |")), "docs/metrics.md lacks a row for {key}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn renaming_a_struct_field_fails_the_check() {
    let dir = scratch_copy("renamed");
    let src_path = dir.join("src/hostpressure.rs");
    let src = std::fs::read_to_string(&src_path).unwrap();
    std::fs::write(&src_path, src.replace("pub load5:", "pub load_five:")).unwrap();

    let out = run_check(&dir);
    assert!(!out.status.success(), "a renamed field must fail the doc check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("load5") || stderr.contains("load_five"), "{stderr}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_doc_missing_a_row_fails_naming_the_field() {
    let dir = scratch_copy("missing-row");
    let doc_path = dir.join("docs/metrics.md");
    let doc = std::fs::read_to_string(&doc_path).unwrap();
    let kept: Vec<&str> = doc.lines().filter(|l| !l.starts_with("| `nproc` |")).collect();
    std::fs::write(&doc_path, kept.join("\n") + "\n").unwrap();

    let out = run_check(&dir);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nproc"));
    let _ = std::fs::remove_dir_all(dir);
}
