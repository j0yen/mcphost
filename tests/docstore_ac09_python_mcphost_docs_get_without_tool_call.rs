//! PRD-mcphost-document-store
//! AC9 -- Given a python-kind tool, When it reads `MCPHOST_DOCS_ENDPOINT`
//! and fetches a document by id using the documented helper, Then it
//! receives the extracted text without a tool call round trip.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn python_tool_reads_a_document_via_mcphost_docs_get() {
    // Same sandbox-dependent skip convention as every other python-kind
    // integration test (tests/python_ac11_secret_redaction.rs etc.).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let put = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "handbook.md", "content": "# Handbook\ncontent"}))
            .await
            .expect("docs put ok"),
    );
    let doc_id = put["id"].as_str().expect("id").to_string();

    let spec = json!({
        "source": "import os, mcphost\ndef main(args):\n    endpoint = os.environ.get('MCPHOST_DOCS_ENDPOINT')\n    text = mcphost.docs.get(args['id'])\n    return {\"endpoint_present\": endpoint is not None and len(endpoint) > 0, \"text\": text}\n",
        "args_schema": {
            "type": "object",
            "properties": {"id": {"type": "string"}},
            "required": ["id"],
        },
    });
    client
        .tools_call("host.tool_publish", json!({"name": "reader", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let result = poll_until_ready(
        &client,
        &format!("{ns}.reader"),
        json!({"id": doc_id}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(
        structured["endpoint_present"],
        json!(true),
        "MCPHOST_DOCS_ENDPOINT must be set in the sandbox: {structured:?}"
    );
    assert_eq!(
        structured["text"],
        json!("# Handbook\ncontent"),
        "mcphost.docs.get must return the extracted text directly: {structured:?}"
    );
}
