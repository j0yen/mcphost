//! PRD-mcphost-trigger-set-idempotent
//! AC4 (P0) — Given a webhook named `x`, When `set(kind: schedule, name:
//! "x")` runs, Then the error class is `trigger_kind_mismatch` and nothing
//! changes.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn set_with_a_mismatched_kind_on_an_existing_name_is_refused() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    let webhook = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "x"}),
            )
            .await
            .expect("trigger.set webhook"),
    );
    let trigger_id = webhook["id"].as_str().expect("id").to_string();
    let url = webhook["url"].as_str().expect("url").to_string();

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "on_payment", "kind": "schedule", "name": "x", "schedule": "*/5 * * * *"}),
        )
        .await
        .expect_err("a schedule set on a name that already names a webhook must be refused");
    assert_eq!(err.error_code.as_deref(), Some("trigger_kind_mismatch"), "{err:?}");
    assert_eq!(err.data["name"], json!("x"), "{:?}", err.data);
    assert_eq!(err.data["existing_kind"], json!("webhook"), "{:?}", err.data);
    assert_eq!(err.data["requested_kind"], json!("schedule"), "{:?}", err.data);

    // Nothing changed: the webhook trigger is exactly as it was.
    let got = extract_structured(
        &client
            .tools_call("host.trigger.get", json!({"id": trigger_id}))
            .await
            .expect("trigger.get"),
    );
    assert_eq!(got["kind"], json!("webhook"), "{got:?}");
    assert_eq!(got["url"], json!(url), "{got:?}");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.list", json!({}))
            .await
            .expect("trigger.list"),
    );
    let triggers = listed["triggers"].as_array().expect("triggers array");
    assert_eq!(triggers.len(), 1, "the refused call must not have created a second trigger: {listed:?}");
}
