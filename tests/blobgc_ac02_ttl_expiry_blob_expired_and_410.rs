//! PRD-mcphost-blob-retention-and-quota
//! AC2 -- Given `host.blob.put {ttl_s: 60}`, When 61 s pass (clock injected),
//! Then `host.blob.get` answers `blob_expired` and the signed URL answers
//! 410; before 60 s both work.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use reqwest::StatusCode;
use serde_json::json;
use std::sync::atomic::Ordering;

fn advance(server: &TestServer, secs: i64) {
    server.state.blob_clock_offset_secs.store(secs, Ordering::Relaxed);
}

#[tokio::test]
async fn ttl_60_lapses_after_61_seconds() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let content = base64::engine::general_purpose::STANDARD.encode(b"short lived");
    let put = extract_structured(
        &client
            .tools_call("host.blob.put", json!({"name": "tmp.txt", "content_base64": content, "ttl_s": 60}))
            .await
            .expect("put"),
    );
    let id = put["id"].as_str().unwrap().to_string();
    let url = put["url"].as_str().unwrap().to_string();
    let expires_at = put["expires_at"].as_i64().expect("put reports expires_at");

    // Before 60 s: both work. The injected offset rides on the real wall
    // clock and `expires_at` was truncated to whole seconds at put time, so an
    // offset of 59 leaves under one second of slack and a second-boundary
    // crossing under CI load flipped the blob to expired (the signed-URL
    // read answered 410, not 200). Test mid-window instead: the slack is
    // ~29 s of real time and no longer depends on scheduler latency. The
    // lapsed side below only gets later with real time, so it is monotone-safe.
    advance(&server, 30);
    let got = extract_structured(&client.tools_call("host.blob.get", json!({"id": id})).await.expect("get at 30 s"));
    assert_eq!(got["id"], json!(id));
    assert_eq!(reqwest::get(&url).await.unwrap().status(), StatusCode::OK);

    // 61 s: both refuse.
    advance(&server, 61);
    let err = client.tools_call("host.blob.get", json!({"id": id})).await.expect_err("expired get");
    assert_eq!(err.data["error_code"], json!("blob_expired"), "{err:?}");
    assert_eq!(err.data["expired_at"], json!(expires_at), "{err:?}");
    let err = client.tools_call("host.blob.url", json!({"id": id})).await.expect_err("expired url mint");
    assert_eq!(err.data["error_code"], json!("blob_expired"));
    assert_eq!(reqwest::get(&url).await.unwrap().status(), StatusCode::GONE);

    // touch revives it until the prune removes the row.
    let touched = extract_structured(
        &client.tools_call("host.blob.touch", json!({"id": id, "ttl_s": 3600})).await.expect("touch"),
    );
    assert!(touched["expires_at"].as_i64().unwrap() > expires_at);
    assert_eq!(reqwest::get(&url).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn ttl_s_beyond_365_days_or_non_positive_is_refused() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC2 bounds").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let content = base64::engine::general_purpose::STANDARD.encode(b"x");
    for ttl in [0_i64, -5, 365 * 86_400 + 1] {
        let err = client
            .tools_call("host.blob.put", json!({"name": "b.txt", "content_base64": content, "ttl_s": ttl}))
            .await
            .expect_err("out-of-range ttl_s");
        assert_eq!(err.data["error_code"], json!("args_invalid"), "ttl {ttl}: {err:?}");
    }
    let ok = client
        .tools_call("host.blob.put", json!({"name": "b.txt", "content_base64": content, "ttl_s": 365 * 86_400}))
        .await;
    assert!(ok.is_ok(), "exactly 365 days is allowed");
}
