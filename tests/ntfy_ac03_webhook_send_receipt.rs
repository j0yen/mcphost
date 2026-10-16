//! PRD-mcphost-notify-user-channel
//! AC3 -- Given a tenant with a webhook channel pointed at a local fixture,
//! When `host.notify.send {text, level:"warn", link}` runs, Then the fixture
//! receives one JSON POST within 5 s whose body carries `text`, `level`,
//! `link`, `notify_id` and no tenant key, and `host.notify.receipts` shows
//! `delivered` with `upstream_status: 200`.

use crate::common;
use axum::{Json, Router, routing::post};
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn start_fixture() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let router = Router::new().route(
        "/hook",
        post(move |Json(body): Json<Value>| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap_or_else(|e| e.into_inner()).push(body);
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve fixture");
    });
    (format!("http://{addr}/hook"), seen)
}

#[tokio::test]
async fn ntfy_ac03() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ntfy ac03 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let (hook_url, seen) = start_fixture().await;

    let added = extract_structured(
        &agent
            .tools_call("host.notify.channel_add", json!({"kind": "webhook", "target": hook_url}))
            .await
            .expect("channel_add"),
    );
    assert!(added["channel_id"].is_string(), "{added}");

    let sent = extract_structured(
        &agent
            .tools_call(
                "host.notify.send",
                json!({"text": "nightly probe failed 3x", "level": "warn", "link": "https://mcphost.dev/v/abc"}),
            )
            .await
            .expect("send"),
    );
    let notify_id = sent["notify_id"].as_str().expect("notify_id").to_string();
    assert_eq!(sent["channels"], json!(1), "{sent}");

    // "Within 5 s" is the timeout itself, not a measured elapsed time.
    let receipt = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let r = extract_structured(
                &agent
                    .tools_call("host.notify.receipts", json!({"notify_id": notify_id}))
                    .await
                    .expect("receipts"),
            );
            if let Some(row) = r["receipts"].as_array().and_then(|rows| rows.iter().find(|row| row["status"] == "delivered")) {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the receipt must read delivered within 5 s");
    assert_eq!(receipt["upstream_status"], json!(200), "{receipt}");
    assert_eq!(receipt["kind"], json!("webhook"), "{receipt}");
    assert_eq!(receipt["notify_id"], json!(notify_id), "{receipt}");

    let bodies = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(bodies.len(), 1, "exactly one POST: {bodies:?}");
    let body = &bodies[0];
    assert_eq!(body["text"], json!("nightly probe failed 3x"));
    assert_eq!(body["level"], json!("warn"));
    assert_eq!(body["link"], json!("https://mcphost.dev/v/abc"));
    assert_eq!(body["notify_id"], json!(notify_id));
    assert!(body["unix"].is_i64(), "{body}");
    let raw = body.to_string();
    assert!(!raw.contains(&key), "the tenant key must never appear in an outbound body: {raw}");
    assert!(!raw.contains("tenant_key"), "{raw}");
}
