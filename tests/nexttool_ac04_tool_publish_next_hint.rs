//! PRD-mcphost-one-next-tool AC4 (P0) — Given a new tenant with zero tools
//! used, When `host.tool_publish` succeeds, Then the result carries
//! `next = {tool: "host.tool_call", why: <=120 chars}` and no other entry.
//!
//! Authenticated via the `tenant_key` argument (not an `Authorization`
//! header): requirement 5 exempts header-authenticated sessions from ever
//! receiving a hint (AC7 covers that case directly), so this AC's own
//! "zero tools used" tenant must reach `host.tool_publish` a different way.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn fresh_tenants_first_tool_publish_carries_the_tool_call_hint() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "tenant_key": key,
            }),
        )
        .await
        .expect("host.tool_publish over tenant_key");
    let structured = extract_structured(&result);

    let next = structured
        .get("next")
        .expect("a zero-tools-used tenant's host.tool_publish must carry next");
    let next_obj = next.as_object().expect("next is an object");
    let mut keys: Vec<&str> = next_obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["tool", "why"],
        "next must carry exactly tool and why, no other entry: {next:?}"
    );
    assert_eq!(next["tool"].as_str(), Some("host.tool_call"), "{next:?}");
    let why = next["why"].as_str().expect("why is a string");
    assert!(!why.is_empty(), "why must not be empty");
    assert!(why.len() <= 120, "why must be at most 120 characters: {} ({why:?})", why.len());
}
