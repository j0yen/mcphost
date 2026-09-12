//! AC2 — Given a `free` tenant with 50 tools (PRD-mcphost-metered-overage
//! AC13's published-numbers alignment raised `tools_max` from 3 to 50; the
//! quota mechanics this test exercises are unchanged), When it calls
//! `host.tool_publish`, Then the error is `quota_exceeded` with
//! `plan: free`, `limit: {name: tools_max, value: 50}`, `used: 50`, and
//! `next: billing.checkout`, and no tool is created.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

fn echo_spec() -> serde_json::Value {
    json!({"schema": {"type": "object"}})
}

fn tool_names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("tool_{i:02}")).collect()
}

#[tokio::test]
async fn fifty_first_publish_is_rejected_with_quota_exceeded() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Free Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in tool_names(50) {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": echo_spec()}),
            )
            .await
            .unwrap_or_else(|e| panic!("publishing {name} must succeed: {} {}", e.code, e.message));
    }

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_51st", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect_err("the 51st tool on the free plan must be rejected");

    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"));
    assert_eq!(err.data["plan"], json!("free"));
    assert_eq!(err.data["limit"]["name"], json!("tools_max"));
    assert_eq!(err.data["limit"]["value"], json!(50));
    assert_eq!(err.data["used"], json!(50));
    assert_eq!(err.data["next"], json!("billing.checkout"));

    // No tool was created for the rejected name.
    let tools = client
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("tool_list");
    let structured = common::extract_structured(&tools);
    let names: Vec<&str> = structured["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 50, "{names:?}");
    assert!(
        !names.iter().any(|n| n.ends_with(".tool_51st")),
        "tool_51st must not have been created: {names:?}"
    );
}

/// Republishing an existing name must not count against the limit -- a
/// tenant already at the cap can still update its own tools.
#[tokio::test]
async fn republish_of_existing_tool_does_not_count_against_the_limit() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Republisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in tool_names(50) {
        client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": echo_spec()}),
            )
            .await
            .expect("initial publish");
    }

    // Republishing tool_00 (already at the cap) must succeed.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_00", "kind": "echo", "spec": echo_spec()}),
        )
        .await
        .expect("republishing an existing tool must not be quota-limited");
}
