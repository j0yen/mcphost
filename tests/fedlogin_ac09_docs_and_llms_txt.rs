//! PRD-mcphost-federated-end-user-login
//! AC9 (P1) — Given `docs/ship-your-mcp-server.md` and `www/llms.txt`,
//! When read, Then the four steps appear with the callback URL and the
//! per-tenant URL, and `scripts/gen-docs-sharing.sh --check` exits 0.

use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const CALLBACK_URL: &str = "https://mcphost.dev/oauth/federation/callback";
const PER_TENANT_URL: &str = "https://mcphost.dev/t/<your-namespace>/mcp";

fn assert_names_the_four_steps(label: &str, text: &str) {
    assert!(text.contains("host.oauth.provider_set"), "{label} must name host.oauth.provider_set");
    assert!(text.contains(CALLBACK_URL), "{label} must name the callback URL");
    assert!(text.contains(PER_TENANT_URL), "{label} must name the per-tenant URL");
    assert!(
        text.to_lowercase().contains("claude.ai") || text.to_lowercase().contains("claude code"),
        "{label} must name a connector step"
    );
}

#[test]
fn ship_your_mcp_server_doc_names_all_four_steps() {
    let text = std::fs::read_to_string(repo_root().join("docs/ship-your-mcp-server.md"))
        .expect("read docs/ship-your-mcp-server.md");
    assert_names_the_four_steps("docs/ship-your-mcp-server.md", &text);
}

#[test]
fn llms_txt_names_all_four_steps_above_sharing_block() {
    let text = std::fs::read_to_string(repo_root().join("www/llms.txt")).expect("read www/llms.txt");
    let sharing_start = text.find("<!-- sharing:start -->").expect("www/llms.txt must have a sharing:start marker");
    let above_sharing = &text[..sharing_start];
    assert_names_the_four_steps("www/llms.txt (above sharing:start)", above_sharing);
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
    assert!(output.status.success(), "gen-docs-sharing.sh --check must exit 0, stderr:\n{stderr}");
}
