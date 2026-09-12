//! AC3 (P0) — Given a Stripe-style `t=,v1=` signature with a timestamp
//! older than the tolerance, When posted, Then 401 with `reason: "timestamp"`.

mod common;
use common::signup;
use serde_json::json;
use sha2::{Digest, Sha256};

fn hmac_sha256_hex(secret: &str, message: &[u8]) -> String {
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
    inner.update(message);
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
async fn stale_stripe_style_timestamp_is_401_naming_the_reason() {
    let server = common::TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "stripe_hook", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call("host.secret_set", json!({"name": "whsec", "value": "whsec_test"}))
        .await
        .expect("secret_set");
    client
        .tools_call(
            "host.trigger.set",
            json!({
                "tool": "stripe_hook",
                "kind": "event",
                "verify": {
                    "scheme": "hmac-sha256",
                    "header": "Stripe-Signature",
                    "secret": "whsec",
                    "timestamp_header": "Stripe-Signature",
                    "tolerance_s": 300,
                },
            }),
        )
        .await
        .expect("trigger.set");

    let body = serde_json::to_vec(&json!({"id": "evt_1"})).unwrap();
    let old_t = mcphost::state::now_unix() - 10_000;
    let signed_payload = [old_t.to_string().as_bytes(), b".", body.as_slice()].concat();
    let sig = hmac_sha256_hex("whsec_test", &signed_payload);

    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{}/hooks/{}/stripe_hook", server.base_url, ns))
        .header("Stripe-Signature", format!("t={old_t},v1={sig}"))
        .body(body)
        .send()
        .await
        .expect("POST /hooks/...");
    assert_eq!(resp.status(), 401);
    let error_body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(error_body["error_code"], json!("signature_invalid"));
    assert_eq!(error_body["reason"], json!("timestamp"));
}
