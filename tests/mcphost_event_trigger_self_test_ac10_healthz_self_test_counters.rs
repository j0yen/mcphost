//! PRD-mcphost-event-trigger-self-test
//! AC10 (P1) — Given three self-tests and one strict-path rejection, When
//! /healthz is read, Then triggers.event_self_tests_total == 3 and
//! triggers.event_self_test_signature_invalid_total == 1.

use crate::common;
use common::{ADMIN_KEY, TestServer, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn healthz_counts_self_tests_and_their_signature_failures() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC10 Tenant").await;
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
    let trigger_id = set["id"].as_str().expect("id").to_string();
    let body = json!({"commits": [{"id": "a"}]});

    let healthz_before = healthz(&server).await;
    let before_tests = healthz_before["triggers"]["event_self_tests_total"].as_i64().unwrap_or(0);
    let before_invalid = healthz_before["triggers"]["event_self_test_signature_invalid_total"]
        .as_i64()
        .unwrap_or(0);

    // Three self-tests (self-signed, all valid) ...
    for _ in 0..3 {
        client
            .tools_call("host.trigger.test", json!({"id": trigger_id, "body": body}))
            .await
            .expect("self-signed trigger.test must succeed");
    }
    // ... and one strict-path rejection.
    client
        .tools_call(
            "host.trigger.test",
            json!({
                "id": trigger_id,
                "body": body,
                "headers": {"X-Hub-Signature-256": "sha256=deadbeef"},
            }),
        )
        .await
        .expect_err("wrong signature must fail");

    let healthz_after = healthz(&server).await;
    let after_tests = healthz_after["triggers"]["event_self_tests_total"].as_i64().unwrap_or(0);
    let after_invalid = healthz_after["triggers"]["event_self_test_signature_invalid_total"]
        .as_i64()
        .unwrap_or(0);

    assert_eq!(after_tests - before_tests, 3, "healthz: {healthz_after}");
    assert_eq!(after_invalid - before_invalid, 1, "healthz: {healthz_after}");
}

async fn healthz(server: &TestServer) -> Value {
    let http = reqwest::Client::new();
    let resp = http
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), 200);
    resp.json().await.expect("json body")
}
