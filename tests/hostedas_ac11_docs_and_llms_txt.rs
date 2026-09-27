//! PRD-mcphost-hosted-authorization-server
//! AC11 (P1) — Given `docs/oauth-connect.md` and `www/llms.txt`, When read,
//! Then both name the claude.ai connector steps, the Claude Code command,
//! and the consent page's key-or-claim-code step, and
//! `scripts/gen-docs-sharing.sh --check` exits 0.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const CLAUDE_CODE_COMMAND: &str = "claude mcp add --transport http mcphost https://mcphost.dev/mcp";

fn assert_names_the_required_steps(label: &str, text: &str) {
    assert!(
        text.contains("claude.ai") && text.to_lowercase().contains("connector"),
        "{label} must name the claude.ai custom connector steps"
    );
    assert!(
        text.contains(CLAUDE_CODE_COMMAND),
        "{label} must contain the exact Claude Code command"
    );
    assert!(
        text.to_lowercase().contains("claim code") && text.to_lowercase().contains("tenant key"),
        "{label} must name the consent page's key-or-claim-code step"
    );
}

#[test]
fn oauth_connect_doc_names_required_steps() {
    let text = std::fs::read_to_string(repo_root().join("docs/oauth-connect.md"))
        .expect("read docs/oauth-connect.md");
    assert_names_the_required_steps("docs/oauth-connect.md", &text);
}

#[test]
fn llms_txt_names_required_steps_above_sharing_block() {
    let text = std::fs::read_to_string(repo_root().join("www/llms.txt")).expect("read www/llms.txt");
    let sharing_start = text.find("<!-- sharing:start -->").expect("www/llms.txt must have a sharing:start marker");
    let above_sharing = &text[..sharing_start];
    assert_names_the_required_steps("www/llms.txt (above sharing:start)", above_sharing);
}

#[test]
fn gen_docs_sharing_check_exits_0() {
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/gen-docs-sharing.sh"))
        .arg("--check")
        .current_dir(repo_root())
        .output()
        .expect("run gen-docs-sharing.sh --check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "gen-docs-sharing.sh --check must exit 0, stderr:\n{stderr}"
    );
}
