//! PRD-mcphost-webhook-inbox
//! AC7 (P0) — Given the same `X-Mcphost-Delivery-Id` twice within 24h,
//! When POSTed, Then the second is 200 and the table has one row.

use crate::common;
use common::{extract_structured, signup};
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
async fn repeated_delivery_id_within_24h_is_200_and_stores_one_row() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
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
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("trigger.set"),
    );
    let url = set["url"].as_str().expect("url").to_string();
    let secret = set["secret"].as_str().expect("secret").to_string();

    let body = json!({"amount": 900});
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = hmac_sha256_hex(&secret, &body_bytes);

    let http = reqwest::Client::new();
    for _ in 0..2 {
        let resp = http
            .post(&url)
            .header("X-Mcphost-Signature", format!("sha256={sig}"))
            .header("X-Mcphost-Delivery-Id", "dlv-001")
            .header("Content-Type", "application/json")
            .body(body_bytes.clone())
            .send()
            .await
            .expect("POST /hook/...");
        assert_eq!(resp.status(), 200);
    }

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "a repeated delivery id within 24h must not re-insert: {rows:?}");
}
