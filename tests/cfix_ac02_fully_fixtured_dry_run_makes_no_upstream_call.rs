//! PRD-mcphost-chain-dry-run AC2 (P0) -- Given fixtures for all 3 steps and a
//! fixture upstream counting requests, When `host.tool_test` runs, Then
//! `executed` is 0, `fixtured` is 3, all three `result` payloads are
//! returned, and the upstream counter is 0.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, publish, signup};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A fixture upstream: answers every request `{"items": [9]}` and counts them.
async fn counting_upstream() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"items":[9]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}/x"), hits)
}

#[tokio::test]
async fn a_fully_fixtured_chain_executes_nothing_and_the_upstream_sees_nothing() {
    let (url, hits) = counting_upstream().await;
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Cfix AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["hop_a", "hop_b", "hop_c"] {
        let spec = json!({"method": "GET", "url": url, "args_schema": {"type": "object"}});
        publish(&client, name, "http", spec).await;
    }
    let spec = json!({
        "steps": [
            {"tool": "hop_a", "args": {}},
            {"tool": "hop_b", "args": {"items": "$.prev.result.items"}},
            {"tool": "hop_c", "args": {"items": "$.prev.result.items"}},
        ]
    });
    publish(&client, "pipeline", "chain", spec).await;

    let fixtures = json!({"1": {"items": [1]}, "2": {"items": [2]}, "3": {"items": [3]}});
    let structured = common::extract_structured(
        &client
            .tools_call("host.tool_test", json!({"name": "pipeline", "args": {}, "fixtures": fixtures}))
            .await
            .expect("host.tool_test with fixtures must succeed"),
    );
    assert_eq!(structured["verdict"], json!("pass"), "{structured}");
    assert_eq!(structured["executed"], json!(0), "{structured}");
    assert_eq!(structured["fixtured"], json!(3), "{structured}");
    assert_eq!(structured["skipped"], json!(0), "{structured}");
    for (i, expected) in [json!({"items": [1]}), json!({"items": [2]}), json!({"items": [3]})].iter().enumerate() {
        assert_eq!(structured["steps"][i]["source"], json!("fixture"), "{structured}");
        assert_eq!(&structured["steps"][i]["result"], expected, "{structured}");
    }
    assert_eq!(structured["steps"][2]["resolved_args"]["items"], json!([2]), "{structured}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "a fixtured step must not reach its upstream");

    // Control: a chain with a fixture for step 1 only does reach the upstream
    // for steps 2-3, so the zero above is the fixtures' doing.
    let control = json!({
        "steps": [
            {"tool": "hop_a", "args": {}},
            {"tool": "hop_b", "args": {"items": "$.prev.result.items"}},
            {"tool": "hop_c", "args": {"x": 1}},
        ]
    });
    publish(&client, "control", "chain", control).await;
    let partial = common::extract_structured(
        &client
            .tools_call(
                "host.tool_test",
                json!({"name": "control", "args": {}, "fixtures": {"1": {"items": [1]}}}),
            )
            .await
            .expect("host.tool_test with a partial fixture"),
    );
    assert_eq!(partial["fixtured"], json!(1), "{partial}");
    assert_eq!(partial["executed"], json!(2), "{partial}");
    assert!(hits.load(Ordering::SeqCst) >= 2, "control: unfixtured steps are dry-run upstream");
}
