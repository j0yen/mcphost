//! PRD-mcphost-trigger-set-idempotent
//! AC1 (P0) — Given no trigger named `webhook:ingest`, When
//! `host.trigger.set(kind: webhook, tool: ingest)` runs twice, Then the
//! second call returns `created: false, changed: []`, `host.trigger.list`
//! shows one row, and the quota counter is 1.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn webhook_set_twice_with_no_name_is_idempotent() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ingest", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let first = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "ingest", "kind": "webhook"}))
            .await
            .expect("first trigger.set"),
    );
    assert_eq!(first["created"], json!(true), "{first:?}");
    assert_eq!(first["name"], json!("webhook:ingest"), "{first:?}");
    let trigger_id = first["id"].as_str().expect("id").to_string();

    let second = extract_structured(
        &client
            .tools_call("host.trigger.set", json!({"tool": "ingest", "kind": "webhook"}))
            .await
            .expect("second trigger.set"),
    );
    assert_eq!(second["created"], json!(false), "{second:?}");
    assert_eq!(second["changed"], json!([]), "{second:?}");
    assert_eq!(second["id"], json!(trigger_id), "same trigger, not a new one: {second:?}");
    assert_eq!(second["name"], json!("webhook:ingest"), "{second:?}");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 1, "two idempotent set calls must leave exactly one row: {listed:?}");

    // Quota counter is 1 (not 2): the free plan's schedules_max is 3
    // (shared by schedule and webhook triggers) -- two more distinct
    // webhooks must still fit, and a fourth distinct webhook must then be
    // refused for quota, proving the double-set above consumed only one
    // slot.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "other_a", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish other_a");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "other_b", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish other_b");

    let second_slot = client
        .tools_call("host.trigger.set", json!({"tool": "other_a", "kind": "webhook", "name": "other_a"}))
        .await
        .expect("second slot should still be free");
    assert_eq!(
        extract_structured(&second_slot)["created"],
        json!(true),
        "the double-set above must have consumed only one quota slot"
    );

    let third_slot = client
        .tools_call("host.trigger.set", json!({"tool": "other_b", "kind": "webhook", "name": "other_b"}))
        .await
        .expect("third slot should still be free");
    assert_eq!(extract_structured(&third_slot)["created"], json!(true));

    let fourth = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "ingest", "kind": "webhook", "name": "fourth"}),
        )
        .await
        .expect_err("a fourth distinct webhook must be refused: only 3 slots exist");
    assert_eq!(fourth.error_code.as_deref(), Some("trigger_quota_exceeded"), "{fourth:?}");
}
