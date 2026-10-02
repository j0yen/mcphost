//! PRD-mcphost-spec-unknown-field-rejection
//! AC7 — Given `scripts/spec-fields-doc-check.sh` at the landing commit,
//! When run, Then it exits 0 and prints one `kind=<k> fields=<n>
//! doc_in_sync=true` line per kind; given a fixture doc with an extra
//! field, Then it exits 1 naming it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_script(repo_root: Option<&std::path::Path>) -> (bool, String, String) {
    let script = crate_root().join("scripts/spec-fields-doc-check.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    if let Some(root) = repo_root {
        cmd.arg(root);
    }
    let output = cmd.output().expect("run spec-fields-doc-check.sh");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn landing_commit_is_in_sync_for_every_kind() {
    let (ok, stdout, stderr) = run_script(None);
    assert!(ok, "script must exit 0 at the landing commit; stderr: {stderr}");
    for kind in ["echo", "http", "python", "wasm"] {
        let needle = format!("kind={kind} ");
        let line = stdout
            .lines()
            .find(|l| l.starts_with(&needle))
            .unwrap_or_else(|| panic!("no output line for kind={kind}: {stdout:?}"));
        assert!(
            line.contains("doc_in_sync=true"),
            "{kind} must report doc_in_sync=true: {line}"
        );
        assert!(
            line.contains("fields="),
            "{kind} must report a fields=<n> count: {line}"
        );
    }
}

#[test]
fn a_doc_with_an_extra_field_fails_naming_it() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-unkfield-ac07-{}-{}",
        std::process::id(),
        mcphost::state::now_unix()
    ));
    let src_kinds = dir.join("src/kinds");
    let docs_kinds = dir.join("docs/kinds");
    fs::create_dir_all(&src_kinds).unwrap();
    fs::create_dir_all(&docs_kinds).unwrap();

    fs::copy(
        crate_root().join("src/kinds/echo.rs"),
        src_kinds.join("echo.rs"),
    )
    .unwrap();
    let doc = fs::read_to_string(crate_root().join("docs/kinds/echo.md")).unwrap();
    let doc = doc.replacen(
        "\"schema\": {",
        "\"bogus_extra_field\": 1,\n  \"schema\": {",
        1,
    );
    fs::write(docs_kinds.join("echo.md"), doc).unwrap();

    let (ok, stdout, stderr) = run_script(Some(&dir));
    assert!(!ok, "a doc with an extra field must fail; stdout: {stdout}");
    assert!(
        stdout.contains("kind=echo") && stdout.contains("doc_in_sync=false"),
        "stdout: {stdout}"
    );
    assert!(
        stderr.contains("bogus_extra_field"),
        "stderr must name the offending field: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
