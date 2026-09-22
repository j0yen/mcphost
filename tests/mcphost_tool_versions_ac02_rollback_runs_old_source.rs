//! PRD-mcphost-tool-versions
//! AC2 (P0) — Given version 2 current, When `host.tool_rollback {version:
//! 1}` is called, Then the next `host.tool_call` runs version 1's source.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn rollback_makes_next_call_run_old_source() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"v1": {"type": "string"}}, "required": ["v1"]}},
            }),
        )
        .await
        .expect("publish v1 should succeed");

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"v2": {"type": "string"}}, "required": ["v2"]}},
            }),
        )
        .await
        .expect("publish v2 should succeed");

    // v2 is current: a call satisfying only v1's schema must fail now.
    let rejected = client
        .tools_call("host.tool_call", json!({"name": "echoer", "args": {"v1": "hi"}}))
        .await;
    assert!(rejected.is_err(), "v1-shaped args must be rejected while v2 is current");

    let rollback = client
        .tools_call("host.tool_rollback", json!({"name": "echoer", "version": 1}))
        .await
        .expect("rollback to version 1 should succeed");
    assert_eq!(extract_structured(&rollback)["version"], json!(1));

    // The next call must run version 1's source (its schema, in this case).
    let call = client
        .tools_call("host.tool_call", json!({"name": "echoer", "args": {"v1": "hi"}}))
        .await
        .expect("v1-shaped args must be accepted after rollback");
    assert_eq!(extract_structured(&call), json!({"v1": "hi"}));

    let history = client
        .tools_call("host.tool_history", json!({"name": "echoer"}))
        .await
        .expect("tool_history should succeed");
    let versions = extract_structured(&history)["versions"].clone();
    let versions = versions.as_array().expect("versions array");
    let current: Vec<_> = versions
        .iter()
        .filter(|v| v["current"] == json!(true))
        .collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["version"], json!(1));
}
