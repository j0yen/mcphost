//! PRD-mcphost-webhook-inbox-one-call
//! AC6 -- Given `www/llms.txt`, When `scripts/www-check.sh` and
//! `handler::llms_txt_tool_names` run, Then a section `## Webhook inbox in
//! one call` names `host.webhook_inbox.create`, and
//! `examples/webhook-inbox/proof.sh` against a local `TestServer` exits 0 in
//! under 30 s.

use crate::common;

use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::handler::llms_txt_tool_names;
use mcphost::kinds::KindRegistry;
use mcphost::sandbox;
use tokio::process::Command;

const LLMS_TXT: &str = include_str!("../www/llms.txt");
const HEADING: &str = "## Webhook inbox in one call";

fn section() -> &'static str {
    let start = LLMS_TXT.find(HEADING).expect("www/llms.txt must have the webhook inbox section");
    let rest = &LLMS_TXT[start + HEADING.len()..];
    let end = rest.find("\n## ").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

#[test]
fn section_names_the_one_create_call_and_it_is_a_live_tool() {
    let section = section();
    assert!(section.contains("host.webhook_inbox.create("), "{section}");
    assert!(
        llms_txt_tool_names(&KindRegistry::with_builtin()).iter().any(|n| n == "host.webhook_inbox.create"),
        "host.webhook_inbox.create must be a registered tool"
    );
    assert!(section.contains("host.msg.wait"), "{section}");
    assert!(section.contains("examples/webhook-inbox/proof.sh"), "{section}");
}

#[tokio::test]
async fn www_check_passes() {
    let output = Command::new("bash")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/www-check.sh"))
        .output()
        .await
        .expect("run scripts/www-check.sh");
    assert!(
        output.status.success(),
        "www-check.sh failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn proof_sh_exits_zero_in_under_thirty_seconds() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/webhook-inbox/proof.sh");

    // Async `Command`: a blocking `output()` would starve the in-process
    // server's own axum task. proof.sh must finish within 30 s.
    let output = crate::deadline_budget!(30_000, {
        Command::new("bash")
            .arg(&script)
            .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
            .output()
            .await
            .expect("run proof.sh")
    });
    let transcript = format!(
        "{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "proof.sh exited non-zero:\n{transcript}");
    assert!(transcript.contains("RESULT: PASS"), "{transcript}");
}
