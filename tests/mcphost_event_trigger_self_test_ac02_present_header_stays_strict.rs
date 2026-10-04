//! PRD-mcphost-event-trigger-self-test
//! AC2 (P0) — Given the same trigger as AC1, When host.trigger.test is
//! called with headers: {"X-Hub-Signature-256": ""} or a wrong value, Then
//! it fails signature_invalid with message "signature in
//! 'X-Hub-Signature-256' did not match" and no run row is created.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

async fn setup(server: &TestServer) -> (common::McpClient, String) {
    let (_ns, key) = signup(&server.base_url, "SelfTest AC2 Tenant").await;
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
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({
                    "tool": "gh_push",
                    "kind": "event",
                    "verify": {
                        "scheme": "hmac-sha256",
                        "header": "X-Hub-Signature-256",
                        "secret": "hook-secret-1",
                        "prefix": "sha256=",
                    },
                }),
            )
            .await
            .expect("trigger.set"),
    );
    (client, set["id"].as_str().expect("id").to_string())
}

async fn assert_run_count(client: &common::McpClient, expected: usize) {
    let runs = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list"),
    );
    assert_eq!(
        runs["runs"].as_array().map(|a| a.len()),
        Some(expected),
        "runs: {runs}"
    );
}

#[tokio::test]
async fn empty_header_value_is_rejected_not_self_signed() {
    let server = TestServer::start().await;
    let (client, trigger_id) = setup(&server).await;
    let body = json!({"commits": [{"id": "a"}, {"id": "b"}, {"id": "c"}]});

    let err = client
        .tools_call(
            "host.trigger.test",
            json!({"id": trigger_id, "body": body, "headers": {"X-Hub-Signature-256": ""}}),
        )
        .await
        .expect_err("an empty configured header must fall through to strict verify and fail");
    assert_eq!(err.error_code.as_deref(), Some("signature_invalid"));
    assert_eq!(err.message, "signature in 'X-Hub-Signature-256' did not match");

    assert_run_count(&client, 0).await;
}

#[tokio::test]
async fn wrong_header_value_is_rejected() {
    let server = TestServer::start().await;
    let (client, trigger_id) = setup(&server).await;
    let body = json!({"commits": [{"id": "a"}, {"id": "b"}, {"id": "c"}]});

    let err = client
        .tools_call(
            "host.trigger.test",
            json!({
                "id": trigger_id,
                "body": body,
                "headers": {"X-Hub-Signature-256": "sha256=deadbeef"},
            }),
        )
        .await
        .expect_err("a wrong configured header value must fail, never auto-upgrade to self-signed");
    assert_eq!(err.error_code.as_deref(), Some("signature_invalid"));
    assert_eq!(err.message, "signature in 'X-Hub-Signature-256' did not match");

    assert_run_count(&client, 0).await;
}
