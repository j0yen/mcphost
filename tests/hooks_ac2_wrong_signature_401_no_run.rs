//! AC2 (P0) — Given the same hmac-sha256 trigger, When the signature is
//! wrong, Then 401 signature_invalid, no run is created, and nothing is
//! logged containing the secret.

mod common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn wrong_signature_is_401_and_creates_no_run() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
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
            json!({
                "tool": "gh_push",
                "kind": "event",
                "verify": {
                    "scheme": "hmac-sha256",
                    "header": "X-Hub-Signature-256",
                    "secret": "gh_hook",
                    "prefix": "sha256=",
                },
            }),
        )
        .await
        .expect("trigger.set");

    let body = serde_json::to_vec(&json!({"ref": "refs/heads/main"})).unwrap();

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hooks/{}/gh_push", server.base_url, ns))
        .header("X-Hub-Signature-256", "sha256=0000000000000000000000000000000000000000000000000000000000000000")
        .body(body)
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 401);
    let error_body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(error_body["error_code"], json!("signature_invalid"));
    assert_eq!(error_body["header"], json!("X-Hub-Signature-256"));
    let dump = error_body.to_string();
    assert!(!dump.contains("s3cr3t"), "response must never leak the secret: {dump}");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "event"}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert!(runs.is_empty(), "a bad signature must create no run: {runs:?}");
}
