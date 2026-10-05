//! PRD-mcphost-event-trigger-self-test
//! AC1 (P0) — Given an event trigger with verify scheme hmac-sha256, header
//! X-Hub-Signature-256, prefix sha256=, secret name hook-secret-1, When
//! host.trigger.test(id, body={"commits":[...]}) is called with no headers,
//! Then the response has status: "queued", test: true,
//! signed.headers["X-Hub-Signature-256"] == "sha256=" + hex(HMAC-SHA256
//! (secret, signed.body)), and host.runs.wait(run_id) reaches done with the
//! tool having received event.body.commits of length 3.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    let key = secret.as_bytes();
    if key.len() > BLOCK_SIZE {
        let hashed = Sha256::digest(key);
        key_block[..hashed.len()].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(body);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    let mut out = String::new();
    for b in outer.finalize() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[tokio::test]
async fn trigger_test_with_no_headers_self_signs_and_the_run_receives_the_body() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC1 Tenant").await;
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

    let body = json!({"commits": [{"id": "a"}, {"id": "b"}, {"id": "c"}]});
    let tested = extract_structured(
        &client
            .tools_call("host.trigger.test", json!({"id": trigger_id, "body": body}))
            .await
            .expect("trigger.test with no headers must self-sign, not fail"),
    );
    assert_eq!(tested["status"], json!("queued"));
    assert_eq!(tested["test"], json!(true));

    let signed_body = tested["signed"]["body"].as_str().expect("signed.body must be a string").to_string();
    let expected_sig = format!("sha256={}", hmac_sha256_hex("s3cr3t", signed_body.as_bytes()));
    assert_eq!(
        tested["signed"]["headers"]["X-Hub-Signature-256"],
        json!(expected_sig),
        "tested: {tested}"
    );

    let run_id = tested["run_id"].as_str().expect("run_id").to_string();
    let waited = extract_structured(
        &client
            .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 5}))
            .await
            .expect("runs.wait"),
    );
    assert_eq!(waited["status"], json!("done"), "run: {waited}");
    assert_eq!(
        waited["result"]["event"]["body"]["commits"]
            .as_array()
            .map(|a| a.len()),
        Some(3),
        "tool must have received event.body.commits of length 3: {waited}"
    );
}
