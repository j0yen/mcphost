//! PRD-mcphost-one-next-tool AC6 (P0) — Given
//! `host.agent.profile_set(hints = false)`, When any later `host.*` call
//! succeeds, Then no `next` field is present.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn hints_false_suppresses_next_on_a_later_call() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
    let client = McpClient::new(&server.base_url);

    let profile_result = client
        .tools_call("host.agent.profile_set", json!({"hints": false, "tenant_key": key}))
        .await
        .expect("host.agent.profile_set(hints: false)");
    let profile = extract_structured(&profile_result);
    assert_eq!(profile["hints"], json!(false), "{profile:?}");

    // A later host.* call that would otherwise be well within the hint
    // table's own reach (a fresh tenant's first host.tool_publish) must
    // still carry no next.
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
        "hints: false must suppress next on every later host.* call: {structured:?}"
    );
}
