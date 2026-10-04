//! PRD-mcphost-event-trigger-self-test
//! AC4 (P0) — Given host.trigger.set(tool, kind="event", verify="github",
//! secret="hook-secret-1"), When host.trigger.get(id) is read, Then verify
//! shows {scheme:"hmac-sha256", header:"X-Hub-Signature-256",
//! prefix:"sha256="}, preset: "github", dedupe_header:
//! "X-GitHub-Delivery", and the secret value is never present.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn github_preset_expands_into_the_documented_shape_with_no_secret_leaked() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC4 Tenant").await;
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
                json!({"tool": "gh_push", "kind": "event", "verify": "github", "secret": "hook-secret-1"}),
            )
            .await
            .expect("trigger.set with the github preset"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let expected_verify = json!({
        "scheme": "hmac-sha256",
        "header": "X-Hub-Signature-256",
        "prefix": "sha256=",
    });
    assert_eq!(set["verify"], expected_verify, "set response: {set}");
    assert_eq!(set["preset"], json!("github"), "set response: {set}");
    assert_eq!(set["dedupe_header"], json!("X-GitHub-Delivery"), "set response: {set}");
    assert!(
        !set.to_string().contains("s3cr3t"),
        "the plaintext secret must never appear in the set response: {set}"
    );

    let got = extract_structured(
        &client
            .tools_call("host.trigger.get", json!({"id": trigger_id}))
            .await
            .expect("trigger.get"),
    );
    assert_eq!(got["verify"], expected_verify, "get response: {got}");
    assert_eq!(got["preset"], json!("github"), "get response: {got}");
    assert_eq!(got["dedupe_header"], json!("X-GitHub-Delivery"), "get response: {got}");
    assert!(
        !got.to_string().contains("s3cr3t"),
        "the plaintext secret must never appear in the get response: {got}"
    );
}
