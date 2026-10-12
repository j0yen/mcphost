//! PRD-mcphost-event-trigger-from-webhook AC2 (P0) — Given a trigger with
//! `verify: github` and a delivery whose signature is wrong, When delivered,
//! Then the answer is 401 with `dead_letter_id`, the letter has
//! `reason_code: signature_mismatch`, and its `headers_json` carries no
//! header outside the allow list.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;

const ALLOW: &[&str] = &["content-type", "user-agent", "x-request-id", "x-hub-signature-256", "x-github-delivery"];

#[tokio::test]
async fn wrong_github_signature_is_a_signature_mismatch_letter_with_redacted_headers() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Edead AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "gh_push", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call("host.secret_set", json!({"name": "gh_hook", "value": "s3cr3t"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.trigger.set",
            json!({"tool": "gh_push", "kind": "event", "verify": "github", "secret": "gh_hook"}),
        )
        .await
        .expect("trigger.set");

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hooks/{}/gh_push", server.base_url, ns))
        .header("X-Hub-Signature-256", "sha256=0000000000000000000000000000000000000000000000000000000000000000")
        .header("Authorization", "Bearer should-never-be-stored")
        .header("Cookie", "session=should-never-be-stored")
        .header("X-Internal-Secret", "should-never-be-stored")
        .header("Content-Type", "application/json")
        .body(json!({"ref": "refs/heads/main"}).to_string())
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 401);
    let refusal: serde_json::Value = resp.json().await.expect("json body");
    let letter_id = refusal["error"]["dead_letter_id"].as_str().expect("dead_letter_id in body");

    let listed = extract_structured(
        &client
            .tools_call("host.trigger.dead_letters", json!({}))
            .await
            .expect("dead_letters"),
    );
    let letters = listed["dead_letters"].as_array().expect("dead_letters array");
    assert_eq!(letters.len(), 1, "{letters:?}");
    assert_eq!(letters[0]["dead_letter_id"], json!(letter_id));
    assert_eq!(letters[0]["reason_code"], json!("signature_mismatch"));

    let headers = letters[0]["headers_json"].as_object().expect("headers_json object");
    assert!(!headers.is_empty(), "the allow-listed headers are kept: {headers:?}");
    for name in headers.keys() {
        assert!(ALLOW.contains(&name.as_str()), "header {name} is outside the allow list");
    }
    assert!(!letters[0].to_string().contains("should-never-be-stored"));
}
