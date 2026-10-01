//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC1 (P0) -- Given the registry at the landing commit, When
//! `scripts/tool-naming-lint.sh` runs, Then it exits 0, lists each
//! documented exception with its reason, and reports
//! `violations=0 aliases=<n>`; given a fixture registering
//! `host.secret_delete`, Then it exits 1 naming `host.secret.delete`.
//!
//! Shells out to the real script (bash + python3, not Rust) rather than
//! reimplementing its regex here -- a passing test must prove the actual
//! script works, not a parallel copy of its logic.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn lint_script() -> PathBuf {
    repo_root().join("scripts/tool-naming-lint.sh")
}

#[test]
fn lint_passes_on_the_real_registry_with_every_exception_listed() {
    let out = Command::new(lint_script())
        .current_dir(repo_root())
        .output()
        .expect("run tool-naming-lint.sh");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "lint should pass on the real registry: stdout={stdout} stderr={stderr}");

    // requirement 1: every documented exception category is listed with a
    // reason -- one representative of each kind is enough to prove the
    // category is covered, not every single name. Note: an alias name
    // itself (e.g. host.tool_publish) never appears as a `Tool::new(...)`
    // literal in src/handler.rs any more -- only its canonical does; the
    // alias is added to the registry programmatically (handler.rs's
    // host_tools() alias loop), so the lint's own "grandfathered alias"
    // branch exists for defensive completeness (a literal re-added by
    // mistake would still be excused, not flagged) rather than firing on
    // today's registry.
    assert!(stdout.contains("exception signup"), "{stdout}");
    assert!(stdout.contains("exception billing.plans"), "{stdout}");
    assert!(stdout.contains("exception host.whoami") && stdout.contains("singleton noun"), "{stdout}");

    // The 20 violators this PRD found and aliased at landing.
    assert!(stdout.contains("violations=0 aliases=20"), "{stdout}");
}

#[test]
fn lint_fails_a_fixture_registering_a_new_violation_and_names_the_fix() {
    let fixture_path = std::env::temp_dir().join(format!(
        "tool-naming-lint-fixture-{}-{}.rs",
        std::process::id(),
        "ac01"
    ));
    std::fs::write(
        &fixture_path,
        "fn fixture() -> Tool {\n    Tool::new(\n        \"host.secret_delete\",\n        \"d\",\n        json!({}),\n    )\n}\n",
    )
    .expect("write fixture");

    let out = Command::new(lint_script())
        .arg(&fixture_path)
        .current_dir(repo_root())
        .output()
        .expect("run tool-naming-lint.sh on the fixture");
    let _ = std::fs::remove_file(&fixture_path);

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(!out.status.success(), "a fixture registering host.secret_delete must fail the lint");
    assert!(stderr.contains("host.secret_delete"), "must name the offending tool: {stderr}");
    assert!(stderr.contains("host.secret.delete"), "must suggest the canonical form: {stderr}");
}
