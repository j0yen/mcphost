//! PRD-mcphost-trigger-set-idempotent
//! AC2 (P0) — Given an existing webhook trigger, When `set` runs with the
//! same name and a different `tool`, Then `changed == ["tool_name"]`, the
//! id and hook URL are unchanged, and a POST to the hook URL runs the new
//! tool.

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
async fn webhook_set_with_new_tool_updates_in_place_and_delivers_to_new_tool() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_payment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish on_payment");
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "on_shipment", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish on_shipment");

    let first = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_payment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("first trigger.set"),
    );
    let trigger_id = first["id"].as_str().expect("id").to_string();
    let url = first["url"].as_str().expect("url").to_string();
    let secret = first["secret"].as_str().expect("secret").to_string();

    let second = extract_structured(
        &client
            .tools_call(
                "host.trigger.set",
                json!({"tool": "on_shipment", "kind": "webhook", "name": "pay"}),
            )
            .await
            .expect("second trigger.set"),
    );
    assert_eq!(second["created"], json!(false), "{second:?}");
    assert_eq!(second["changed"], json!(["tool_name"]), "{second:?}");
    assert_eq!(second["id"], json!(trigger_id), "id must be unchanged: {second:?}");
    assert_eq!(second["url"], json!(url), "hook URL must be unchanged: {second:?}");
    assert_eq!(second["tool"], json!("on_shipment"), "{second:?}");
    assert!(
        second.get("secret").is_none() || second["secret"].is_null(),
        "an update must never re-disclose the secret: {second:?}"
    );

    // The hook URL still accepts the ORIGINAL secret (requirement 2:
    // updating never regenerates it) and now runs the new tool.
    let body = json!({"tracking": "abc123"});
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

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(got["tool"], json!("on_shipment"), "the POST must have run the new tool: {got:?}");
            break;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
