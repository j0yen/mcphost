//! PRD-mcphost-blob-retention-and-quota
//! AC7 -- Given `notify_before_s: 86400` and three blobs expiring in 12 h,
//! When the hourly tick runs twice, Then exactly one `blob.expiring
//! {count: 3}` message is in the inbox.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

async fn expiring_messages(client: &McpClient) -> Vec<Value> {
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["data"]["kind"] == json!("blob.expiring"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn two_ticks_send_one_notice_for_three_blobs_expiring_in_12_hours() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC7").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.blob.policy", json!({"notify_before_s": 86_400})).await.expect("policy");
    let content = base64::engine::general_purpose::STANDARD.encode(b"x");
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let put = extract_structured(
            &client
                .tools_call("host.blob.put", json!({"name": name, "content_base64": content, "ttl_s": 12 * 3600}))
                .await
                .expect("put"),
        );
        ids.push(put["id"].as_str().unwrap().to_string());
    }
    // A fourth blob far outside the lead time is not in the notice.
    client
        .tools_call("host.blob.put", json!({"name": "later.txt", "content_base64": content, "ttl_s": 30 * 86_400}))
        .await
        .expect("put later");

    assert_eq!(mcphost::blobs::expiry_notice_tick(&server.state).await.unwrap(), 1);
    assert_eq!(mcphost::blobs::expiry_notice_tick(&server.state).await.unwrap(), 0, "second tick sends nothing");

    let notices = expiring_messages(&client).await;
    assert_eq!(notices.len(), 1, "exactly one blob.expiring message: {notices:?}");
    let data = &notices[0]["data"];
    assert_eq!(data["count"], json!(3));
    assert_eq!(data["bytes"], json!(3));
    let mut got: Vec<String> = data["ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    got.sort();
    ids.sort();
    assert_eq!(got, ids);
    assert!(data["earliest_expires_at"].as_i64().unwrap() > mcphost::state::now_unix());

    // A day later the next tick may send the next notice (never sooner).
    server.state.blob_clock_offset_secs.store(23 * 3600, Ordering::Relaxed);
    assert_eq!(mcphost::blobs::expiry_notice_tick(&server.state).await.unwrap(), 0, "still inside the day");
}

#[tokio::test]
async fn no_policy_or_nothing_expiring_means_no_notice() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Blobgc AC7 none").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let content = base64::engine::general_purpose::STANDARD.encode(b"x");
    client
        .tools_call("host.blob.put", json!({"name": "a.txt", "content_base64": content, "ttl_s": 60}))
        .await
        .expect("put");
    assert_eq!(mcphost::blobs::expiry_notice_tick(&server.state).await.unwrap(), 0, "notify_before_s defaults to off");
    client.tools_call("host.blob.policy", json!({"notify_before_s": 30})).await.expect("policy");
    assert_eq!(mcphost::blobs::expiry_notice_tick(&server.state).await.unwrap(), 0, "60 s is beyond a 30 s lead time");
    assert!(expiring_messages(&client).await.is_empty());
}
