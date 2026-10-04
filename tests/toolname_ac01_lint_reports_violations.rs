//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC1 — Given the registry at the landing commit, When
//! `scripts/tool-naming-lint.sh` runs, Then it exits 0, lists each
//! documented exception with its reason, and reports
//! `violations=0 aliases=<n>`; given a fixture registering
//! `host.secret_delete`, Then it exits 1 naming `host.secret.delete`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_script(repo_root: Option<&std::path::Path>) -> (bool, String, String) {
    let script = crate_root().join("scripts/tool-naming-lint.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script);
    if let Some(root) = repo_root {
        cmd.arg(root);
    }
    let output = cmd.output().expect("run tool-naming-lint.sh");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn landing_commit_has_zero_violations_and_lists_exceptions() {
    let (ok, stdout, stderr) = run_script(None);
    assert!(ok, "script must exit 0 at the landing commit; stderr: {stderr}");

    for (name, _) in [
        ("signup", ()),
        ("billing.*", ()),
        ("host.whoami", ()),
        ("host.redeem", ()),
        ("host.quickstart", ()),
        ("host.usage", ()),
        ("host.changelog", ()),
        ("host.export", ()),
        ("host.progress", ()),
    ] {
        let needle = format!("exception={name} reason=\"");
        assert!(
            stdout.lines().any(|l| l.starts_with(&needle)),
            "stdout must list {name} as a documented exception with a reason: {stdout}"
        );
    }

    let summary = stdout
        .lines()
        .find(|l| l.starts_with("violations="))
        .unwrap_or_else(|| panic!("no violations=.. summary line: {stdout:?}"));
    assert_eq!(summary, "violations=0 aliases=20", "stdout: {stdout}");
}

#[test]
fn a_fixture_registering_an_underscore_name_fails_naming_the_canonical_form() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-toolname-ac01-{}-{}",
        std::process::id(),
        mcphost::state::now_unix()
    ));
    let src = dir.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("handler.rs"),
        r#"
fn host_tools() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.secret_delete",
            "delete a secret",
            schema(),
        ),
    ]
}
"#,
    )
    .unwrap();

    let (ok, stdout, stderr) = run_script(Some(&dir));
    assert!(!ok, "a bare underscore violator must fail; stdout: {stdout}");
    assert!(
        stdout.contains("violations=1"),
        "stdout must report exactly one violation: {stdout}"
    );
    assert!(
        stderr.contains("host.secret_delete") && stderr.contains("host.secret.delete"),
        "stderr must name both the offending name and its canonical form: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
