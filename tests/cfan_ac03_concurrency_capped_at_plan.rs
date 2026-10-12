//! PRD-mcphost-chain-fanout-and-join AC3 (P0) -- Given 10 items, a free-plan
//! tenant, and a fixture tool that records concurrent requests, When called,
//! Then the maximum concurrent requests observed is 4 and all 10 items
//! complete.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A fixture upstream that holds each request 150 ms and records the peak
/// number in flight.
async fn gauge_upstream() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let peak_out = peak.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (now, peak) = (now.clone(), peak.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let in_flight = now.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(in_flight, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                now.fetch_sub(1, Ordering::SeqCst);
                let body = r#"{"ok":true}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}/x?n={{{{ n }}}}"), peak_out)
}

#[tokio::test]
async fn ten_items_on_the_free_plan_never_exceed_four_in_flight() {
    let (url, peak) = gauge_upstream().await;
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "source", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "probe", "http", json!({"method": "GET", "url": url, "args_schema": {"type": "object"}})).await;
    let chain = publish(
        &client,
        "sweep",
        "chain",
        json!({"steps": [
            {"tool": "source", "args": {"items": "$.input.items"}},
            {"tool": "probe", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"},
        ]}),
    )
    .await;

    let items: Vec<u32> = (0..10).collect();
    let call = client.tools_call(&chain, json!({"items": items})).await.expect("fan-out call succeeds");
    let result = extract_structured(&call)["result"].clone();
    assert_eq!(result["ok"], json!(10), "all ten items complete: {result}");
    assert_eq!(result["failed_count"], json!(0), "{result}");
    assert_eq!(peak.load(Ordering::SeqCst), 4, "the free plan's concurrent_calls_per_tenant is 4");
}
