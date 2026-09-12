//! AC7 (P0) — Given `host.tool_remove` on a tool with two schedules, When
//! it runs, Then both triggers are disabled and the result says
//! `triggers_disabled: 2`.

use crate::common;
use common::{TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;

#[tokio::test]
async fn tool_remove_disables_its_triggers_and_reports_the_count() {
    let server = TestServer::start().await;
    let (_ns, key, _tenant_id) = signup_and_make_pro(&server, "AC7 Tenant", "cus_ac7").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let mut trigger_ids = Vec::new();
    for schedule in ["*/5 * * * *", "*/6 * * * *"] {
        let set = extract_structured(
            &client
                .tools_call("host.trigger.set", json!({"tool": "pinger", "schedule": schedule}))
                .await
                .expect("trigger.set"),
        );
        trigger_ids.push(set["id"].as_str().expect("id").to_string());
    }

    let removed = extract_structured(
        &client
            .tools_call("host.tool_remove", json!({"name": "pinger"}))
            .await
            .expect("tool_remove"),
    );
    assert_eq!(removed["triggers_disabled"], json!(2), "{removed:?}");

    for id in trigger_ids {
        let got = extract_structured(
            &client
                .tools_call("host.trigger.get", json!({"id": id}))
                .await
                .expect("trigger.get"),
        );
        assert_eq!(got["enabled"], json!(false), "{got:?}");
    }
}
