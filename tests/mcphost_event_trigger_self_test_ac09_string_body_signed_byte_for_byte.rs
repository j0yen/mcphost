//! PRD-mcphost-event-trigger-self-test
//! AC9 (P1) — Given body passed as the string {"a":1, "b":2} (with a
//! space), When self-signed, Then signed.body is exactly that string and
//! the signature verifies over it.

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
async fn a_string_body_is_signed_and_returned_byte_for_byte() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "SelfTest AC9 Tenant").await;
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

    const RAW_BODY: &str = r#"{"a":1, "b":2}"#;
    let tested = extract_structured(
        &client
            .tools_call("host.trigger.test", json!({"id": trigger_id, "body": RAW_BODY}))
            .await
            .expect("trigger.test with a string body"),
    );
    let signed_body = tested["signed"]["body"].as_str().expect("signed.body").to_string();
    assert_eq!(signed_body, RAW_BODY, "signed.body must be the exact string, spacing included");

    let expected_sig = format!("sha256={}", hmac_sha256_hex("s3cr3t", RAW_BODY.as_bytes()));
    assert_eq!(
        tested["signed"]["headers"]["X-Hub-Signature-256"],
        json!(expected_sig),
        "tested: {tested}"
    );
}
