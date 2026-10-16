//! PRD-mcphost-notify-user-channel-b
//! AC3 -- Given a webhook fixture that returns 500 twice then 200, When one
//! notify is sent, Then the receipt ends `delivered` with `attempts: 3`;
//! Given a fixture that always returns 500, Then it ends `failed` with
//! `attempts: 3`.

use crate::common;
use axum::{Router, http::StatusCode, routing::post};
use common::{EnvGuard, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A fixture answering 500 for its first `fail_first` hits, 200 after.
async fn start_fixture(fail_first: usize) -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let router = Router::new().route(
        "/hook",
        post(move || {
            let counter = counter.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) < fail_first {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    StatusCode::OK
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve fixture");
    });
    (format!("http://{addr}/hook"), hits)
}

/// Send one notify through a fresh webhook channel and wait for a receipt in
/// `want` status; returns the receipt row.
async fn send_and_settle(agent: &McpClient, hook_url: &str, want: &str) -> Value {
    agent
        .tools_call("host.notify.channel_add", json!({"kind": "webhook", "target": hook_url}))
        .await
        .expect("channel_add");
    let sent = extract_structured(&agent.tools_call("host.notify.send", json!({"text": "retry me", "level": "warn"})).await.expect("send"));
    let notify_id = sent["notify_id"].as_str().expect("notify_id").to_string();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let r = extract_structured(&agent.tools_call("host.notify.receipts", json!({"notify_id": notify_id})).await.expect("receipts"));
            if let Some(row) = r["receipts"].as_array().and_then(|rows| rows.iter().find(|row| row["status"] == want)) {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the receipt must reach {want}"))
}

#[tokio::test]
async fn ntfyb_ac03() {
    let _backoff = EnvGuard::set("MCPHOST_NOTIFY_RETRY_BACKOFF_MS", "20,40").await;
    let server = TestServer::start().await;

    let (_ns, key) = signup(&server.base_url, "ntfyb ac03 flaky tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let (url, hits) = start_fixture(2).await;
    let row = send_and_settle(&agent, &url, "delivered").await;
    assert_eq!(row["attempts"], json!(3), "{row}");
    assert_eq!(row["upstream_status"], json!(200), "{row}");
    assert_eq!(hits.load(Ordering::SeqCst), 3, "500, 500, 200");

    let (_ns, key) = signup(&server.base_url, "ntfyb ac03 dead tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let (url, hits) = start_fixture(usize::MAX).await;
    let row = send_and_settle(&agent, &url, "failed").await;
    assert_eq!(row["attempts"], json!(3), "{row}");
    assert_eq!(row["upstream_status"], json!(500), "{row}");
    assert_eq!(hits.load(Ordering::SeqCst), 3, "exactly three attempts, no more");
}
