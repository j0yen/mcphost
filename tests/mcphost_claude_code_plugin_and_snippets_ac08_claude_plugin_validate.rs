//! AC8 (PRD-mcphost-claude-code-plugin-and-snippets) — Given the `claude`
//! CLI on PATH, When `claude plugin validate plugin/` runs, Then exit 0;
//! Given it is absent, Then the test prints `skipped: claude cli absent`
//! and exits 0.

use std::path::Path;
use std::process::Command;

#[test]
fn claude_plugin_validate_exits_0_or_skips_cleanly() {
    let claude_present = Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !claude_present {
        println!("skipped: claude cli absent");
        return;
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("claude")
        .arg("plugin")
        .arg("validate")
        .arg("plugin/")
        .current_dir(manifest_dir)
        .output()
        .expect("claude cli was detected present but failed to spawn");

    assert!(
        output.status.success(),
        "claude plugin validate plugin/ failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
