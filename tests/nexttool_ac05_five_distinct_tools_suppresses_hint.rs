//! PRD-mcphost-one-next-tool AC5 (P0) — Given a tenant that has used five
//! distinct tools, When any `host.*` call succeeds, Then no `next` field
//! is present.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sixth_call_after_five_distinct_tools_carries_no_next() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::new(&server.base_url);

    // Five distinct host.* tools, each a real successful call so
    // `host_tool_usage` really does grow past the cutoff.
    for name in [
        "host.whoami",
        "host.tool_list",
        "host.secret_list",
        "host.usage",
        "host.agent.whoami",
    ] {
        client
            .tools_call(name, json!({"tenant_key": key}))
            .await
            .unwrap_or_else(|e| panic!("{name} must succeed: {} {}", e.code, e.message));
    }

    // The sixth distinct host.* call -- a brand new tool, never used
    // before, which would otherwise be well within the hint table's own
    // reach -- must carry no next at all.
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
    assert!(
        structured.get("next").is_none(),
        "a tenant past the five-distinct-tool cutoff must never see next: {structured:?}"
    );
}
