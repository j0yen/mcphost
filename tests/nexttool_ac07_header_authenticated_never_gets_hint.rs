//! PRD-mcphost-one-next-tool AC7 (P0) — Given a header-authenticated
//! session on a new tenant, When `host.tool_publish` succeeds, Then no
//! `next` field is present.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn header_authenticated_tool_publish_carries_no_next() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
            }),
        )
        .await
        .expect("header-authenticated host.tool_publish");
    let structured = extract_structured(&result);
    assert!(
        structured.get("next").is_none(),
        "a header-authenticated session must never receive next: {structured:?}"
    );
}
