//! AC4 -- Given a draining server, When a new `/mcp` POST arrives, Then it is
//! refused at the socket (connection refused or 503), and `/healthz` returns
//! 503 with `state: draining`.

use crate::common;
use crate::gdrain;
use crate::serve;

use std::time::Duration;

#[tokio::test]
async fn gdrain_ac04() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let mut fx = gdrain::start("ac04", 6, 1).await;
    // One call that keeps the drain open for several seconds.
    let calls = fx.fire(1, 5).await;
    let http = reqwest::Client::new();
    let base = fx.server.base_url();

    let server: &mut serve::ServeChild = &mut fx.server;
    server.terminate();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let post = http
        .post(format!("{base}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await;
    match post {
        Err(e) => assert!(e.is_connect(), "must be refused at the socket: {e:?}"),
        Ok(resp) => assert_eq!(resp.status().as_u16(), 503, "a new /mcp POST during the drain"),
    }

    let health = http.get(format!("{base}/healthz")).send().await.expect("healthz answers while draining");
    assert_eq!(health.status().as_u16(), 503);
    let body: serde_json::Value = health.json().await.expect("healthz json");
    assert_eq!(body["state"], serde_json::json!("draining"), "{body}");
    assert!(body["restart_id"].is_string(), "{body}");

    // The accepted call still returns its real result, then the process exits 0.
    for call in calls {
        let value = call.await.expect("call task").expect("accepted call finishes");
        assert_eq!(common::extract_structured(&value)["ok"], serde_json::json!(true));
    }
    let status = server.wait_exit(Duration::from_secs(10)).expect("process exits");
    assert_eq!(status.code(), Some(0));
}
