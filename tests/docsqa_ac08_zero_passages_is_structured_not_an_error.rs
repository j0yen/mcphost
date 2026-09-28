//! PRD-mcphost-docs-qa-recipe
//! AC8 -- Given a search returning zero passages, When `ask_docs` runs,
//! Then it returns a structured "no passages found" result, not an error.

use crate::common;

use common::{TempDataDir, TestServer, extract_structured, python_kind_registry};
use serde_json::json;

#[tokio::test]
async fn zero_passages_is_a_structured_result_not_an_error() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let signup_client = common::McpClient::new(&server.base_url);
    let signup_resp = signup_client
        .tools_call("signup", json!({"name": "docsqa-ac08-owner"}))
        .await
        .expect("signup");
    let signup_struct = extract_structured(&signup_resp);
    let ns = signup_struct["tenant"].as_str().expect("tenant").to_string();
    let key = signup_struct["key"].as_str().expect("key").to_string();
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let ask_docs_source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/docs-qa/ask_docs.py"),
    )
    .expect("read ask_docs.py");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ask_docs", "kind": "python", "spec": {"source": ask_docs_source}}),
        )
        .await
        .expect("publish ask_docs");

    // No document was ever put for this tenant, so any query returns zero
    // passages -- the search index has nothing to find.
    let call = client
        .tools_call(&format!("{ns}.ask_docs"), json!({"query": "anything at all"}))
        .await
        .unwrap_or_else(|e| panic!("ask_docs must succeed, not error, on zero passages: {} {}", e.code, e.message));

    let structured = extract_structured(&call);
    assert_eq!(structured["found"], json!(false), "structured: {structured}");
    assert_eq!(structured["passages"], json!([]), "structured: {structured}");
    assert_eq!(structured["message"], json!("no passages found"), "structured: {structured}");
}
