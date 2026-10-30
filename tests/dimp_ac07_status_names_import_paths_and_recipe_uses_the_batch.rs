//! PRD-mcphost-docs-import-and-stated-boundary
//! AC7 -- Given `host.docs.status`, When it is read, Then `import_paths` equals
//! `["content", "content_base64", "put_many"]`, `put_many_max` is 50 and
//! `doc_max_bytes` equals the configured cap; and `examples/docs-qa/docs-qa.sh`
//! against the test host loads its corpus in one `put_many` call with the
//! receipt's `tool_calls` <= 4.

use crate::common;
use crate::dimp;
use crate::docs_qa;

use common::{EnvGuard, TempDataDir, TestServer, extract_structured, python_kind_registry};
use serde_json::json;

#[tokio::test]
async fn dimp_ac07_status_names_the_import_paths_and_the_configured_cap() {
    let _cap = EnvGuard::set("MCPHOST_DOC_MAX_BYTES", "4096").await;
    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac7 status").await;

    let status = extract_structured(&client.tools_call("host.docs.status", json!({})).await.expect("status"));
    assert_eq!(status["import_paths"], json!(["content", "content_base64", "put_many"]), "status: {status}");
    assert_eq!(status["put_many_max"], json!(50), "status: {status}");
    assert_eq!(status["doc_max_bytes"], json!(4096), "status: {status}");
}

#[tokio::test]
async fn dimp_ac07_docs_qa_recipe_loads_its_corpus_in_one_put_many_call() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("dimp-ac07");

    let run = docs_qa::run_docs_qa(&endpoint, &["--receipt-dir", receipt_dir.to_str().expect("utf8 path")], &[]).await;
    assert!(run.success, "docs-qa.sh must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt = run.receipt();
    assert_eq!(receipt["call_counts"]["host.docs.put_many"], json!(1), "one batch call loads the corpus: {receipt}");
    assert!(receipt["call_counts"].get("host.docs.put").is_none(), "no one-at-a-time puts: {receipt}");
    let tool_calls = receipt["tool_calls"].as_i64().expect("receipt.tool_calls");
    assert!(tool_calls <= 4, "tool_calls must be <= 4, got {tool_calls}: {receipt}");
    assert_eq!(tool_calls, 1, "the corpus load is one call: {receipt}");
    // The shipped docs-qa AC2 bound still holds on the published tool's own calls.
    assert!(receipt["quota"]["tool_calls"].as_i64().expect("quota.tool_calls") <= 10, "receipt: {receipt}");
    assert_eq!(receipt["quota"]["documents"], json!(8));

    std::fs::remove_dir_all(&receipt_dir).ok();
}
