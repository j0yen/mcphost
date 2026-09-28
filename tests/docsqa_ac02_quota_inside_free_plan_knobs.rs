//! PRD-mcphost-docs-qa-recipe
//! AC2 -- Given the receipt, When its quota section is read, Then
//! documents <= 8, chunks <= 100, tool calls <= 10 -- inside free-plan
//! knobs.
//!
//! "tool calls" here is the receipt's own count of calls to the recipe's
//! one published tool (`ask_docs`), not every `host.*` control-plane call
//! docs-qa.sh makes along the way (signup/put/status/publish) -- the same
//! "only the published tool's own calls count toward `calls_per_day`"
//! accounting `examples/uptime-probes/proof.sh`'s "288 of 500 calls/day"
//! narrative uses (schedule firings only, not the setup calls around
//! them).

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};

#[tokio::test]
async fn quota_section_stays_inside_free_plan_knobs() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("ac02");

    let run = docs_qa::run_docs_qa(
        &endpoint,
        &["--receipt-dir", receipt_dir.to_str().expect("utf8 path")],
        &[],
    )
    .await;
    assert!(run.success, "docs-qa.sh must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt = run.receipt();
    let quota = &receipt["quota"];

    let documents = quota["documents"].as_i64().expect("quota.documents");
    assert!(documents <= 8, "documents must be <= 8, got {documents}: {receipt}");

    let chunks = quota["chunks"].as_i64().expect("quota.chunks");
    assert!(chunks > 0, "chunks must be > 0 (the corpus must actually get indexed): {receipt}");
    assert!(chunks <= 100, "chunks must be <= 100, got {chunks}: {receipt}");

    let tool_calls = quota["tool_calls"].as_i64().expect("quota.tool_calls");
    assert!(tool_calls <= 10, "tool_calls must be <= 10, got {tool_calls}: {receipt}");

    std::fs::remove_dir_all(&receipt_dir).ok();
}
