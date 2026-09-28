//! PRD-mcphost-docs-qa-recipe
//! AC5 -- Given the plugin `docs-qa` command, When invoked against the
//! test host, Then it runs the same script and prints the receipt path.

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};

const COMMAND_MD: &str = include_str!("../plugin/commands/docs-qa.md");

/// Pulls the fenced ` ```bash ... ``` ` block out of the command file
/// verbatim -- this is the literal text the plugin actually runs when a
/// human types `/docs-qa <endpoint>`, quoting, argument order, and all.
fn bash_block_from_command_md() -> String {
    let start = COMMAND_MD.find("```bash\n").expect("plugin/commands/docs-qa.md must have a ```bash block") + "```bash\n".len();
    let end = COMMAND_MD[start..].find("```").expect("```bash block must be closed");
    COMMAND_MD[start..start + end].to_string()
}

#[tokio::test]
async fn plugin_command_runs_docs_qa_sh_and_prints_the_receipt_path() {
    assert!(COMMAND_MD.contains("description:"), "plugin/commands/docs-qa.md must have frontmatter");
    let block = bash_block_from_command_md();
    assert!(
        block.contains("examples/docs-qa/docs-qa.sh"),
        "the command's own script block must invoke examples/docs-qa/docs-qa.sh: {block}"
    );

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("ac05");

    // "Invoked against the test host" -- run the command's own bash block,
    // verbatim, with `$1` bound to the endpoint exactly as a human's
    // `/docs-qa <endpoint>` would bind it. A bad quote, a swapped argument,
    // or stray text around `docs-qa.sh` in the command file executes here
    // and fails the run, rather than being silently ignored in favor of a
    // separately hand-built invocation.
    let run = docs_qa::run_command_block(
        &block,
        &endpoint,
        &[("MCPHOST_RECEIPT_DIR", receipt_dir.to_str().expect("utf8 path"))],
    )
    .await;
    assert!(run.success, "the command's bash block must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt_path = run.receipt_path();
    assert!(
        std::path::Path::new(&receipt_path).exists(),
        "the printed receipt path must actually exist: {receipt_path}"
    );
    assert!(
        run.stdout.lines().any(|l| l.starts_with("RECEIPT: ")),
        "the command's own script must print a RECEIPT: line: {}",
        run.stdout
    );

    std::fs::remove_dir_all(&receipt_dir).ok();
}
