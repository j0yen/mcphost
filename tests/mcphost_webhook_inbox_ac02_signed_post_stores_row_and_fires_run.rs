//! PRD-mcphost-webhook-inbox
//! AC2 (P0) — Given a body signed with the secret, When POSTed to the url,
//! Then 200, one row in `inbox_pay`, and one run of `on_payment` whose
//! argument equals the row.

use crate::common;
use common::{extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

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
async fn signed_delivery_is_accepted_stored_and_fires_a_run_with_matching_args() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
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

    let body = json!({"amount": 4200, "currency": "usd"});
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
    assert_eq!(resp.status(), 200);
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let run_id = accepted["run_id"].as_str().expect("run_id in response").to_string();

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "exactly one row in inbox_pay: {rows:?}");
    assert_eq!(rows[0]["body"], body, "stored row's body must equal what was POSTed");

    // Poll for the run to finish (echo-kind tools run essentially
    // instantly) and confirm its argument equals the stored row.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(got["trigger"], json!("webhook"));
            assert_eq!(
                got["result"], rows[0],
                "the run's argument must equal the stored inbox row: {got:?}"
            );
            break;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
