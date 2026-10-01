//! PRD-mcphost-sandbox-bridge-discoverability
//! AC7 (P1) -- Given `scripts/sandbox-api-doc-check.sh` at the landing
//! commit, When run, Then it exits 0 and reports `modules=<n>
//! docs_in_sync=3`; given a fixture doc with one module removed, Then it
//! exits 1 naming the missing module and file.
//!
//! No server, no sandbox: this just shells out to the script itself,
//! against the real checkout first and then a fixture tree.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_script(root: &std::path::Path) -> std::process::Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/sandbox-api-doc-check.sh"))
        .arg(root)
        .output()
        .expect("sandbox-api-doc-check.sh must be runnable via bash")
}

#[test]
fn passes_against_the_real_checkout() {
    let output = run_script(&repo_root());
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().starts_with("modules=") && stdout.contains("docs_in_sync=3"),
        "expected 'modules=<n> docs_in_sync=3', got: {stdout}"
    );
}

#[test]
fn fails_naming_the_missing_module_and_file_on_drift() {
    let fixture = std::env::temp_dir().join(format!(
        "mcphost-sandbox-api-doc-check-fixture-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let real_root = repo_root();

    // Mirror only what the script reads: the runner source (untouched,
    // still the real registration list) and the three docs, one of them
    // with `mcphost.table` struck out -- same drift shape AC7 names.
    fs::create_dir_all(fixture.join("src/kinds")).unwrap();
    fs::create_dir_all(fixture.join("www")).unwrap();
    fs::create_dir_all(fixture.join("docs/kinds")).unwrap();
    fs::create_dir_all(fixture.join("plugin/skills/mcphost")).unwrap();

    fs::copy(
        real_root.join("src/kinds/python.rs"),
        fixture.join("src/kinds/python.rs"),
    )
    .expect("copy src/kinds/python.rs");
    fs::copy(real_root.join("www/llms.txt"), fixture.join("www/llms.txt")).expect("copy www/llms.txt");
    fs::copy(
        real_root.join("plugin/skills/mcphost/SKILL.md"),
        fixture.join("plugin/skills/mcphost/SKILL.md"),
    )
    .expect("copy SKILL.md");

    let real_doc = fs::read_to_string(real_root.join("docs/kinds/python.md")).expect("read python.md");
    let drifted_doc = real_doc.replace("mcphost.table", "mcphost.TABLE_REMOVED_BY_FIXTURE");
    assert_ne!(real_doc, drifted_doc, "the fixture must actually remove every mcphost.table mention");
    fs::write(fixture.join("docs/kinds/python.md"), drifted_doc).expect("write fixture doc");

    let output = run_script(&fixture);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(!output.status.success(), "the fixture must fail: stderr={stderr}");
    assert!(stderr.contains("mcphost.table"), "stderr must name the missing module: {stderr}");
    assert!(
        stderr.contains("docs/kinds/python.md"),
        "stderr must name the file missing it: {stderr}"
    );

    let _ = fs::remove_dir_all(&fixture);
}
