//! AC1 (PRD-mcphost-claude-code-plugin-and-snippets) — Given the built
//! tree, When `tests/plugin_assets.sh` runs, Then it exits 0 and prints
//! one line per check.

use std::path::Path;
use std::process::Command;

#[test]
fn plugin_assets_script_exits_0_and_prints_one_line_per_check() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("bash")
        .arg("tests/plugin_assets.sh")
        .current_dir(manifest_dir)
        .output()
        .expect("failed to spawn tests/plugin_assets.sh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "tests/plugin_assets.sh must exit 0; stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 10,
        "expected at least 10 checks printed, got {}: {stdout}",
        lines.len()
    );
    for line in &lines {
        assert!(
            line.starts_with("ok - ") || line.starts_with("FAIL - "),
            "unexpected line from tests/plugin_assets.sh: {line}"
        );
        assert!(
            line.starts_with("ok - "),
            "a check failed: {line}\nfull output: {stdout}"
        );
    }
}
