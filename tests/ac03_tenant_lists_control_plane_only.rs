//! AC3 — Given a tenant key, When the client sends `tools/list`, Then it
//! lists the `host.*` control-plane tools and no other tenant's tools.

use crate::common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn tenant_tools_list_is_host_star_with_no_foreign_tools() {
    let server = TestServer::start().await;

    // Another tenant publishes a tool first, to prove it never leaks.
    let (other_ns, other_key) = signup(&server.base_url, "Other Tenant").await;
    let other_client = McpClient::with_bearer(&server.base_url, &other_key);
    other_client
        .tools_call(
            "host.tool_publish",
            serde_json::json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("other tenant publishes hello");

    let (tenant_ns, key) = signup(&server.base_url, "Fresh Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();

    for expected in [
        "host.whoami",
        "host.tool_publish",
        "host.tool_list",
        "host.tool_remove",
        "host.tool_logs",
        "host.usage",
        "host.secret_set",
        "host.secret_list",
    ] {
        assert!(
            names.contains(&expected),
            "missing control-plane tool {expected} in {names:?}"
        );
    }
    assert!(
        !names.iter().any(|n| n.starts_with(&format!("{other_ns}."))),
        "must not see {other_ns}'s tools: {names:?}"
    );
    assert!(
        !names.contains(&"signup"),
        "authenticated tenant should not need signup listed"
    );
    let _ = tenant_ns;
}
