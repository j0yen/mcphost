//! PRD-mcphost-document-store
//! AC1 -- Given an empty tenant, When `host.docs.put {name: "a.md",
//! content: "# hi"}` runs, Then it returns `version: 1`, `seq: 1`, and
//! `status` shows `documents: 1` and `watermark: 1`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn first_put_on_an_empty_tenant_returns_version1_seq1_and_status_matches() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let put = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "a.md", "content": "# hi"}))
            .await
            .expect("put ok"),
    );
    assert_eq!(put["version"], json!(1), "put result: {put:?}");
    assert_eq!(put["seq"], json!(1), "put result: {put:?}");
    assert!(put["id"].as_str().is_some(), "put result must carry an id: {put:?}");

    let status = extract_structured(
        &client
            .tools_call("host.docs.status", json!({}))
            .await
            .expect("status ok"),
    );
    assert_eq!(status["documents"], json!(1), "status: {status:?}");
    assert_eq!(status["watermark"], json!(1), "status: {status:?}");
}
