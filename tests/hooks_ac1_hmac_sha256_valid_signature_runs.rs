//! AC1 (P0) — Given an event trigger with `hmac-sha256` over secret `s`,
//! When a POST arrives with a correct `X-Hub-Signature-256`, Then the
//! response is 202 with a `run_id` within 500 ms, and the run finishes with
//! the tool having received `event.body`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
    // Recompute independently of `mcphost::billing::hmac_sha256` (which the
    // production code under test also calls) via the same construction, so
    // this test doesn't just check the implementation against itself --
    // it's the textbook HMAC definition, block size 64 for SHA-256.
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
async fn valid_hmac_sha256_signature_is_accepted_and_tool_receives_event_body() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
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
    assert_eq!(set["url"], json!(format!("{}/hooks/{}/gh_push", server.base_url, ns)));
    assert_eq!(set["unverified"], json!(false));

    let body = serde_json::to_vec(&json!({"ref": "refs/heads/main"})).unwrap();
    let sig = hmac_sha256_hex("s3cr3t", &body);

    let http = reqwest::Client::new();
    let start = Instant::now();
    let resp = http
        .post(format!("{}/hooks/{}/gh_push", server.base_url, ns))
        .header("X-Hub-Signature-256", format!("sha256={sig}"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("POST /hooks/...");
    let elapsed = start.elapsed();
    assert_eq!(resp.status(), 202);
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    let accepted: serde_json::Value = resp.json().await.expect("json body");
    let run_id = accepted["run_id"].as_str().expect("run_id").to_string();

    // The executor runs `echo`-kind tools essentially instantly; poll
    // briefly for the run to finish rather than asserting on the
    // still-`queued` snapshot.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get"),
        );
        if got["status"] == json!("done") {
            assert_eq!(
                got["result"]["event"]["body"],
                json!({"ref": "refs/heads/main"}),
                "tool must have received event.body: {got:?}"
            );
            assert_eq!(got["trigger"], json!("event"));
            break;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished: {got:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
