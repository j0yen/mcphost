//! AC18 (P1) — Given a tenant that just published a tool, When it sends
//! `tools/list` within 60s, Then the result carries `ttlMs: 0`; after
//! 60s, `ttlMs` is at least 30000.

mod common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn ttl_is_zero_right_after_publish_and_positive_once_steady() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TTL Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // Before any publish, the tenant is already "steady" (no recent change).
    let tools = client
        .tools_list()
        .await
        .expect("tools/list before publish");
    assert!(
        tools["ttlMs"].as_u64().unwrap_or(0) >= 30_000,
        "ttlMs should be the steady-state value before any publish: {tools}"
    );

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let tools = client
        .tools_list()
        .await
        .expect("tools/list right after publish");
    assert_eq!(
        tools["ttlMs"].as_u64(),
        Some(0),
        "ttlMs must be 0 within 60s of a publish: {tools}"
    );
}
