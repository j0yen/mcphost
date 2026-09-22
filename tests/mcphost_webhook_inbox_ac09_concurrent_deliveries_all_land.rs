//! PRD-mcphost-webhook-inbox
//! AC9 (P0) — Given 100 concurrent signed deliveries to one hook, When they
//! complete, Then the table has 100 rows and 100 runs exist.

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

const N: usize = 100;

#[tokio::test]
async fn one_hundred_concurrent_deliveries_all_land_as_rows_and_runs() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
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

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let url = url.clone();
        let secret = secret.clone();
        handles.push(tokio::spawn(async move {
            let body = json!({"amount": i});
            let body_bytes = serde_json::to_vec(&body).unwrap();
            let sig = hmac_sha256_hex(&secret, &body_bytes);
            let http = reqwest::Client::new();
            let resp = http
                .post(&url)
                .header("X-Mcphost-Signature", format!("sha256={sig}"))
                .header("Content-Type", "application/json")
                .body(body_bytes)
                .send()
                .await
                .expect("POST /hook/...");
            resp.status()
        }));
    }
    for handle in handles {
        let status = handle.await.expect("task joined");
        assert_eq!(status, 200);
    }

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), N, "all {N} concurrent deliveries must land as rows");

    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "webhook", "limit": 200}))
            .await
            .expect("runs.list"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), N, "all {N} concurrent deliveries must have a run");
}
