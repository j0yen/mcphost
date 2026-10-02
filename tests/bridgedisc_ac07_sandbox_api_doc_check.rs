//! PRD-mcphost-sandbox-bridge-discoverability
//! AC7 (P1) -- Given `scripts/sandbox-api-doc-check.sh` at the landing
//! commit, When run, Then it exits 0 and reports `modules=<n>
//! docs_in_sync=3`; given a fixture doc with one module removed, Then it
//! exits 1 naming the missing module and file.
//!
//! Runs the real script against a scratch copy of the files it touches
//! (never the repo's own on-disk copies), same convention
//! `tests/sharedcall_ac05_gen_docs_sharing_check.rs` uses for
//! `gen-docs-sharing.sh`.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const TOUCHED: &[&str] = &[
    "scripts/sandbox-api-doc-check.sh",
    "src/kinds/python.rs",
    "docs/kinds/python.md",
    "www/llms.txt",
    "plugin/skills/mcphost/SKILL.md",
];

fn scratch_copy(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-bridgedisc-ac07-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    for rel in TOUCHED {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().expect("parent dir")).expect("create scratch subdir");
        std::fs::copy(repo_root().join(rel), &dest).unwrap_or_else(|e| panic!("copy {rel} into scratch: {e}"));
    }
    dir
}

fn run_check(dir: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
        .output()
        .expect("run sandbox-api-doc-check.sh")
}

#[test]
fn landing_commit_passes_reporting_modules_and_docs_in_sync() {
    let dir = scratch_copy("clean");
    let output = run_check(&dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "sandbox-api-doc-check.sh must exit 0 on the committed files, stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("docs_in_sync=3"),
        "must report docs_in_sync=3 for the three target docs: {stdout}"
    );
    let modules_line = stdout
        .lines()
        .find(|l| l.starts_with("modules="))
        .unwrap_or_else(|| panic!("must report a modules=<n> line: {stdout}"));
    let n: u32 = modules_line
        .trim_start_matches("modules=")
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("modules=<n> must be a number: {modules_line}"));
    assert!(n > 0, "modules=<n> must be positive: {modules_line}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_fixture_doc_with_one_module_removed_fails_naming_it_and_the_file() {
    let dir = scratch_copy("missing-module");
    let doc_path = dir.join("docs/kinds/python.md");
    let original = std::fs::read_to_string(&doc_path).expect("read scratch doc");

    // Remove just the mcphost.lineage bullet line from the marked block --
    // everything else (including the ## mcphost.lineage prose section)
    // stays, so this exercises exactly "one module missing from the
    // table", not a mangled file.
    let edited: String = original
        .lines()
        .filter(|l| !l.trim_start().starts_with("- `mcphost.lineage`"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_ne!(edited, original, "the fixture edit must actually remove a line");
    std::fs::write(&doc_path, &edited).expect("write fixture doc");

    let output = run_check(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "a doc missing a module must fail the check");
    assert!(
        stderr.contains("mcphost.lineage"),
        "stderr must name the missing module: {stderr}"
    );
    assert!(
        stderr.contains("docs/kinds/python.md"),
        "stderr must name the file it's missing from: {stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
