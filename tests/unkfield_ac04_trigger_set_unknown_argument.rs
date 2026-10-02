//! PRD-mcphost-spec-unknown-field-rejection
//! AC4 — Given `host.trigger.set` called with an argument key the tool
//! does not define, When dispatched, Then the error class is
//! `unknown_argument` with `data.known` equal to the registered
//! properties and no trigger is created.

use std::collections::BTreeSet;

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

async fn registered_properties(client: &McpClient, tool_name: &str) -> BTreeSet<String> {
    let listed = client.tools_list().await.expect("tools/list ok");
    let tool = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(tool_name))
        .unwrap_or_else(|| panic!("{tool_name} not in tools/list"));
    tool["inputSchema"]["properties"]
        .as_object()
        .expect("properties object")
        .keys()
        .cloned()
        .collect()
}

#[tokio::test]
async fn unknown_argument_is_refused_and_creates_no_trigger() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Unkfield AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "hello",
                "schedule": "* * * * *",
                "notreal": "bogus",
            }),
        )
        .await
        .expect_err("an undefined argument key must be refused");

    assert_eq!(err.error_code.as_deref(), Some("unknown_argument"));

    let known: BTreeSet<String> = err
        .data
        .get("known")
        .and_then(Value::as_array)
        .expect("data.known is an array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let registered = registered_properties(&client, "host.trigger.set").await;
    assert_eq!(
        known, registered,
        "data.known must equal host.trigger.set's own registered properties"
    );

    let listed = client
        .tools_call("host.trigger.list", json!({}))
        .await
        .expect("host.trigger.list ok");
    let triggers = common::extract_structured(&listed)["triggers"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        triggers.is_empty(),
        "a refused host.trigger.set must not have created a trigger: {triggers:?}"
    );
}
