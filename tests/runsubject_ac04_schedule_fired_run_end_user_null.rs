//! PRD-mcphost-runs-end-user-subject
//! AC4 (P0) — Given a schedule-fired run, When read, Then `end_user` is
//! null.

use crate::common;
use common::{extract_structured, signup_and_make_pro};
use serde_json::{Value, json};

#[tokio::test]
async fn schedule_fired_run_has_no_end_user() {
    let server = common::TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "AC4 Tenant", "cus_ac4").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "schedule": "0 0 1 1 *"}),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let fired = extract_structured(
        &client
            .tools_call("host.trigger.fire", json!({"id": trigger_id}))
            .await
            .expect("trigger.fire"),
    );
    let run_id = fired["run_id"].as_str().expect("run_id").to_string();

    let got = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(got["trigger"], json!("schedule"), "{got}");
    assert_eq!(got["end_user"], Value::Null, "schedule-fired run must have no end user: {got}");
}
