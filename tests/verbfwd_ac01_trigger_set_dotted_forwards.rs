//! PRD-mcphost-tool-call-host-verb-forward
//! AC1 — Given a tenant with no tool named `trigger_set` and a published
//! tool `inbox_handler`, When `host.tool_call name="trigger.set"
//! args={tool:"inbox_handler", kind:"event", name:"inbox_hook"}` runs, Then
//! the trigger is created, the response carries `forwarded_to:
//! "host.trigger.set"` and `client_tool: "host_trigger_set"`, and
//! `host.trigger.list` shows `inbox_hook`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn dotted_trigger_set_forwards_and_creates_trigger() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "inbox_handler", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish inbox_handler");

    let call = client
        .tools_call(
            "host.tool_call",
            json!({
                "name": "trigger.set",
                "args": {
                    "tool": "inbox_handler",
                    "kind": "event",
                    "name": "inbox_hook",
                    "verify": {"scheme": "none", "allow_unverified": true},
                },
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("host.tool_call name=\"trigger.set\" must forward: {} {}", e.code, e.message));
    let structured = extract_structured(&call);

    assert_eq!(
        structured["forwarded_to"], "host.trigger.set",
        "forwarded call must carry forwarded_to: {structured}"
    );
    assert_eq!(
        structured["client_tool"], "host_trigger_set",
        "forwarded call must carry client_tool: {structured}"
    );
    assert_eq!(structured["created"], json!(true), "the trigger must actually be created: {structured}");

    let list = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("host.trigger.list"),
    );
    let names: Vec<&str> = list["triggers"]
        .as_array()
        .expect("triggers array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"inbox_hook"),
        "host.trigger.list must show inbox_hook: {names:?}"
    );
}
