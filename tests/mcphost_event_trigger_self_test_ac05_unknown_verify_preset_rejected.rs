//! PRD-mcphost-event-trigger-self-test
//! AC5 (P0) — Given host.trigger.set(kind="event", verify="paypal"), When
//! called, Then it fails trigger_invalid naming verify and listing github,
//! stripe as the presets.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn unknown_verify_preset_names_the_field_and_the_known_presets() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gh_push", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call("host.secret_set", json!({"name": "hook-secret-1", "value": "s3cr3t"}))
        .await
        .expect("secret_set");

    let err = client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "gh_push", "kind": "event", "verify": "paypal", "secret": "hook-secret-1"}),
        )
        .await
        .expect_err("an unknown verify preset must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("trigger_invalid"));
    assert_eq!(err.data["field"], json!("verify"), "error data: {:?}", err.data);
    assert!(err.message.contains("github, stripe"), "message: {}", err.message);
}
