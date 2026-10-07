//! PRD-mcphost-tool-call-host-verb-forward
//! AC3 — Given a tenant that owns a tool literally named `trigger_set`,
//! When `host.tool_call name="trigger_set"` runs, Then the tenant tool runs
//! and nothing is forwarded.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_tenant_tool_named_trigger_set_shadows_the_verb() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "trigger_set", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish a tenant tool literally named trigger_set");

    let call = client
        .tools_call("host.tool_call", json!({"name": "trigger_set", "args": {"msg": "hi"}}))
        .await
        .unwrap_or_else(|e| panic!("calling the tenant's own trigger_set must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&call);

    assert_eq!(
        structured,
        json!({"msg": "hi"}),
        "the tenant's own echo tool must run untouched by forwarding: {structured}"
    );
    assert!(
        structured.get("forwarded_to").is_none() && structured.get("client_tool").is_none(),
        "a tenant-owned tool call must never carry forwarding fields: {structured}"
    );

    let list = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("host.trigger.list"),
    );
    assert_eq!(
        list["triggers"].as_array().expect("triggers array").len(),
        0,
        "nothing must be forwarded to host.trigger.set: {list}"
    );
}
