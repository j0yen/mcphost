//! PRD-mcphost-event-trigger-self-test
//! AC3 (P0) — Given a self-signed test response, When a client POSTs
//! signed.body verbatim with signed.headers to the trigger's public url,
//! Then POST /hooks/... answers 202 with a new run id (the signature
//! verifies on the live path too).

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn replaying_the_self_signed_envelope_at_the_public_url_is_accepted() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "SelfTest AC3 Tenant").await;
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
    let url = set["url"].as_str().expect("url").to_string();
    assert_eq!(url, format!("{}/hooks/{}/gh_push", server.base_url, ns));

    let body = json!({"commits": [{"id": "a"}, {"id": "b"}, {"id": "c"}]});
    let tested = extract_structured(
        &client
            .tools_call("host.trigger.test", json!({"id": trigger_id, "body": body}))
            .await
            .expect("trigger.test with no headers must self-sign"),
    );
    let self_test_run_id = tested["run_id"].as_str().expect("run_id").to_string();
    let signed_body = tested["signed"]["body"].as_str().expect("signed.body").to_string();
    let signed_sig = tested["signed"]["headers"]["X-Hub-Signature-256"]
        .as_str()
        .expect("signed.headers[X-Hub-Signature-256]")
        .to_string();

    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .header("X-Hub-Signature-256", signed_sig)
        .header("Content-Type", "application/json")
        .body(signed_body)
        .send()
        .await
        .expect("POST /hooks/... with the replayed signed envelope");
    assert_eq!(resp.status(), 202, "the self-signed envelope must verify on the live path too");
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let live_run_id = accepted["run_id"].as_str().expect("run_id").to_string();
    assert_ne!(
        live_run_id, self_test_run_id,
        "the live POST must create its own new run, not reuse the test run"
    );
}
