//! AC5 (PRD-mcphost-signup-kill-switch-and-source) — Given the pause file
//! exists, When an existing tenant calls `host.tool_call`,
//! `host.trigger.fire`, and `billing.status`, Then all succeed as before.

use crate::common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn pause_file_does_not_affect_authenticated_calls() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) =
        signup_and_make_pro(&server, "AC5 Tenant", "cus_ac5_pause").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish before pause");
    let set = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "pinger", "schedule": "0 0 1 1 *"}))
            .await
            .expect("trigger.set before pause"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    // Now pause signups.
    std::fs::write(server.state.signup_pause.path(), "paused for ac5\n").expect("write pause file");

    let call_result = client
        .tools_call("host.tool_call", json!({"name": "pinger", "args": {}}))
        .await
        .expect("host.tool_call must succeed while paused");
    assert_eq!(extract_structured(&call_result), json!({}), "{call_result:?}");

    let fired = client
        .tools_call("host.trigger.fire", json!({"id": trigger_id}))
        .await
        .expect("host.trigger.fire must succeed while paused");
    assert_eq!(extract_structured(&fired)["manual"], json!(true), "{fired:?}");

    let status = client
        .tools_call("billing.status", json!({}))
        .await
        .expect("billing.status must succeed while paused");
    assert_eq!(extract_structured(&status)["plan"], json!("pro"), "{status:?}");
}
