//! AC7 (P0) — Given `host.trigger.test(id, body, headers)` with a valid
//! signature, When called, Then a run is created marked `test: true` and
//! the response carries its id.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use sha2::{Digest, Sha256};

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
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
async fn trigger_test_with_valid_signature_creates_a_test_marked_run() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
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
                        "secret": "gh_hook",
                        "prefix": "sha256=",
                    },
                }),
            )
            .await
            .expect("trigger.set"),
    );
    let trigger_id = set["id"].as_str().expect("id").to_string();

    let body = json!({"ref": "refs/heads/main"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = hmac_sha256_hex("s3cr3t", &body_bytes);

    let tested = extract_structured(
        &client
            .tools_call(
                "host.trigger.test",
                json!({
                    "id": trigger_id,
                    "body": body,
                    "headers": {"X-Hub-Signature-256": format!("sha256={sig}")},
                }),
            )
            .await
            .expect("trigger.test"),
    );
    assert_eq!(tested["test"], json!(true));
    let run_id = tested["run_id"].as_str().expect("run_id carried in the response").to_string();

    let got = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(got["test"], json!(true), "the persisted run must be marked test: {got:?}");

    // A wrong secret's own signature fails the same way a real delivery
    // would, naming the header it checked (AC7's own "verifies... exactly
    // as POST /hooks/... would").
    let bad = client
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
    assert_eq!(bad.error_code.as_deref(), Some("signature_invalid"));
}
