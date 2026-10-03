//! PRD-mcphost-trigger-set-idempotent
//! AC3 (P0) — Given a schedule trigger, When `set` runs with the same name
//! and a new cron, Then `next_unix` reflects the new cron and `changed`
//! includes `schedule`.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn schedule_set_with_new_cron_updates_next_unix_in_place() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let first = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "kind": "schedule", "name": "daily", "schedule": "0 1 * * *"}),
            )
            .await
            .expect("first trigger.set"),
    );
    assert_eq!(first["created"], json!(true), "{first:?}");
    let trigger_id = first["id"].as_str().expect("id").to_string();
    let next_unix_1 = first["next_unix"].as_i64().expect("next_unix");

    let second = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "pinger", "kind": "schedule", "name": "daily", "schedule": "0 2 * * *"}),
            )
            .await
            .expect("second trigger.set"),
    );
    assert_eq!(second["created"], json!(false), "{second:?}");
    assert_eq!(second["id"], json!(trigger_id), "same trigger, not a new one: {second:?}");
    let changed = second["changed"].as_array().expect("changed array");
    assert!(
        changed.iter().any(|v| v == "schedule"),
        "changed must include \"schedule\": {changed:?}"
    );
    let next_unix_2 = second["next_unix"].as_i64().expect("next_unix");
    assert_ne!(next_unix_2, next_unix_1, "next_unix must reflect the new cron: {second:?}");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 1, "the update must not have minted a second trigger: {listed:?}");
    assert_eq!(triggers[0]["next_unix"], json!(next_unix_2), "{listed:?}");
}
