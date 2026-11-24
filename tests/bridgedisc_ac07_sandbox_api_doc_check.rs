//! contract: reads docs/sandbox-api.md
//! PRD-mcphost-sandbox-bridge-discoverability
//! AC7 (P1) -- Given the tree at the landing commit, When
//! `mcphost gen-docs kinds --check` runs (the sandbox-API block moved there
//! from the regex script, PRD-mcphost-kind-docs-from-registry-b-v2 R2), Then
//! it exits 0; given a fixture doc with one module removed, Then it exits 1
//! naming the file and the `mcphost-sandbox-api` block.
//!
//! Runs the real binary against a scratch copy of the files it reads (never
//! the repo's own on-disk copies).

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const TOUCHED: &[&str] = &["www/llms.txt", "plugin/skills/mcphost/SKILL.md"];

fn scratch_copy(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-bridgedisc-ac07-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(dir.join("docs/kinds")).expect("create scratch docs/kinds");
    for entry in std::fs::read_dir(repo_root().join("docs/kinds")).expect("read docs/kinds") {
        let entry = entry.expect("dir entry");
        std::fs::copy(entry.path(), dir.join("docs/kinds").join(entry.file_name())).expect("copy kind page");
    }
    for rel in TOUCHED {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().expect("parent dir")).expect("create scratch subdir");
        std::fs::copy(repo_root().join(rel), &dest).unwrap_or_else(|e| panic!("copy {rel} into scratch: {e}"));
    }
    dir
}

fn run_check(dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "kinds", "--check"])
        .current_dir(dir)
        .output()
        .expect("run mcphost gen-docs kinds --check")
}

#[test]
fn landing_commit_passes_gen_docs_kinds_check() {
    let dir = scratch_copy("clean");
    let output = run_check(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "gen-docs kinds --check must exit 0 on the committed files, stderr:\n{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_fixture_doc_with_one_module_removed_fails_naming_the_file_and_block() {
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
    assert_eq!(output.status.code(), Some(1), "a doc missing a module must fail the check: {stderr}");
    assert!(stderr.contains("docs/kinds/python.md"), "stderr must name the file it's missing from: {stderr}");
    assert!(stderr.contains("mcphost-sandbox-api"), "stderr must name the block: {stderr}");

    std::fs::remove_dir_all(&dir).ok();
}
