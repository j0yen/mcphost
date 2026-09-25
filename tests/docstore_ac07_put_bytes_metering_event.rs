//! PRD-mcphost-document-store
//! AC7 -- Given a `put` of 1 000 bytes, When metering is read, Then a
//! `docs.put_bytes` event of 1 000 exists for the tenant.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn put_emits_a_docs_put_bytes_metering_event_of_the_content_size() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Docs AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let content = "a".repeat(1000);
    let put = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "metered.txt", "content": content}))
            .await
            .expect("put ok"),
    );
    assert_eq!(put["bytes"], json!(1000), "put result: {put:?}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let quantities = server
        .state
        .db
        .document_usage_event_quantities(tenant.id, "docs.put_bytes".to_string())
        .await
        .expect("read docs.put_bytes events");
    assert!(
        quantities.contains(&1000),
        "expected a docs.put_bytes event of 1000 for this tenant, got {quantities:?}"
    );
}
