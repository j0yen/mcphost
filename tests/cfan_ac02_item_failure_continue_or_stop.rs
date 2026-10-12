//! PRD-mcphost-chain-fanout-and-join AC2 (P0) -- Given item index 1 fails
//! with `upstream_error` and the step has `on_error: continue`, When called,
//! Then `items[1]` is `null`, `failed` is `[{index: 1, error_code:
//! "upstream_error", message}]`, and the chain ends with `failed_steps: [2]`;
//! Given the default `on_error`, Then the chain fails with
//! `compose_fanout_item_failed` and `data.index` 1.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A fixture upstream: `GET /x?n=<n>` answers 200 `{"ok": <n>}`, except
/// `n=1`, which answers 500.
async fn flaky_upstream() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let first = head.lines().next().unwrap_or_default().to_string();
                let (status, body) = if first.contains("n=1 ") {
                    ("500 Internal Server Error", r#"{"error":"boom"}"#.to_string())
                } else {
                    ("200 OK", r#"{"ok":true}"#.to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/x?n={{{{ n }}}}")
}

async fn setup(on_error: Option<&str>) -> (TestServer, McpClient, String) {
    let url = flaky_upstream().await;
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfan AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "source", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "probe", "http", json!({"method": "GET", "url": url, "args_schema": {"type": "object"}})).await;
    let mut step = json!({"tool": "probe", "args": {"n": "$.item"}, "for_each": "$.prev.result.items"});
    if let Some(mode) = on_error {
        step["on_error"] = json!(mode);
    }
    let chain = publish(
        &client,
        "sweep",
        "chain",
        json!({"steps": [{"tool": "source", "args": {"items": "$.input.items"}}, step]}),
    )
    .await;
    (server, client, chain)
}

#[tokio::test]
async fn continue_reports_the_failed_item_and_the_failed_step() {
    let (_server, client, chain) = setup(Some("continue")).await;
    let call = client.tools_call(&chain, json!({"items": [0, 1, 2]})).await.expect("continue tolerates item failures");
    let structured = extract_structured(&call);
    assert_eq!(structured["failed_steps"], json!([2]), "{structured}");
    let result = &structured["result"];
    assert_eq!(result["items"][1], json!(null), "{result}");
    assert!(result["items"][0].is_object() && result["items"][2].is_object(), "{result}");
    let failed = result["failed"].as_array().expect("failed array");
    assert_eq!(failed.len(), 1, "{result}");
    assert_eq!(failed[0]["index"], json!(1), "{result}");
    assert_eq!(failed[0]["error_code"], json!("upstream_error"), "{result}");
    assert!(failed[0]["message"].is_string(), "{result}");
    assert_eq!(result["ok"], json!(2), "{result}");
    assert_eq!(result["failed_count"], json!(1), "{result}");
}

#[tokio::test]
async fn the_default_on_error_fails_the_chain_naming_the_index() {
    let (_server, client, chain) = setup(None).await;
    let err = client
        .tools_call(&chain, json!({"items": [0, 1, 2]}))
        .await
        .expect_err("the default on_error stops the chain");
    assert_eq!(err.error_code.as_deref(), Some("compose_fanout_item_failed"), "{err:?}");
    assert_eq!(err.data["index"], json!(1), "{:?}", err.data);
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["item_error_code"], json!("upstream_error"), "{:?}", err.data);
}
