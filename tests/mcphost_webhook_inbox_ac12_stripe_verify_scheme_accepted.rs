//! PRD-mcphost-webhook-inbox
//! AC12 (P2) — Given `verify=stripe` and a body signed the Stripe way with
//! the hook secret, When POSTed, Then it is accepted.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

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
async fn stripe_style_signature_over_the_hook_secret_is_accepted() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC12 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let set = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay", "verify": "stripe"}),
            )
            .await
            .expect("trigger.set"),
    );
    let url = set["url"].as_str().expect("url").to_string();
    let secret = set["secret"].as_str().expect("secret").to_string();
    assert_eq!(set["verify"], json!("stripe"));

    let body = json!({"type": "payment_intent.succeeded"});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let signed_payload = [t.to_string().as_bytes(), b".", body_bytes.as_slice()].concat();
    let sig = hmac_sha256_hex(&secret, &signed_payload);

    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .header("Stripe-Signature", format!("t={t},v1={sig}"))
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .expect("POST /hook/...");
    assert_eq!(resp.status(), 200);

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "the Stripe-signed delivery must be accepted and stored: {rows:?}");
}
