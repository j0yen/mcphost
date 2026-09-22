//! PRD-mcphost-webhook-inbox
//! AC6 (P0) — Given the hook paused, When a signed body is POSTed, Then
//! 200, a row, and no run; `resume` creates no run; `replay <row>` creates
//! exactly one.

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

async fn webhook_run_count(client: &common::McpClient) -> usize {
    let listed = extract_structured(
        &client
            .tools_call("host.runs.list", json!({"trigger": "webhook"}))
            .await
            .expect("runs.list"),
    );
    listed["runs"].as_array().expect("runs array").len()
}

#[tokio::test]
async fn paused_hook_stores_but_does_not_fire_resume_is_silent_replay_fires_once() {
    let server = common::TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Tenant").await;
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
    let trigger_id = set["id"].as_str().expect("id").to_string();
    let url = set["url"].as_str().expect("url").to_string();
    let secret = set["secret"].as_str().expect("secret").to_string();

    client
        .tools_call("host.trigger.pause", json!({"id": trigger_id}))
        .await
        .expect("trigger.pause");

    let body = json!({"amount": 500});
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
        .expect("POST /hook/... while paused");
    assert_eq!(resp.status(), 200);
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    assert!(accepted["run_id"].is_null(), "a paused hook must fire no run: {accepted:?}");
    let row_id = accepted["row_id"].as_i64().expect("row_id in response");

    let queried = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "inbox_pay"}))
            .await
            .expect("state.query"),
    );
    let rows = queried["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "the delivery must still be stored while paused: {rows:?}");

    assert_eq!(webhook_run_count(&client).await, 0, "no run while paused");

    client
        .tools_call("host.trigger.resume", json!({"id": trigger_id}))
        .await
        .expect("trigger.resume");
    assert_eq!(webhook_run_count(&client).await, 0, "resume must not auto-replay");

    let replayed = extract_structured(
        &client
            .tools_call("host.trigger.replay", json!({"id": trigger_id, "row_id": row_id}))
            .await
            .expect("trigger.replay"),
    );
    assert!(replayed["run_id"].as_str().is_some(), "replay must return a run_id: {replayed:?}");
    assert_eq!(webhook_run_count(&client).await, 1, "replay must create exactly one run");
}
