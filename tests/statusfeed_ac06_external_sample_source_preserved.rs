//! PRD-mcphost-status-feed AC6 (P0): given
//! `admin.status.sample {component: "mcp", ok: false, latency_ms: 0,
//! source: "deploy-probe"}`, when posted 5 times, then the component's
//! state reflects the external samples and `source` is preserved per row.

use crate::common;

use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn external_samples_drive_component_state_and_keep_their_source() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    for _ in 0..5 {
        admin
            .tools_call(
                "admin.status.sample",
                json!({"component": "mcp", "ok": false, "latency_ms": 0, "source": "deploy-probe"}),
            )
            .await
            .expect("admin.status.sample");
    }

    let rows = server
        .state
        .db
        .recent_status_samples("mcp".to_string(), 5)
        .await
        .expect("recent_status_samples");
    assert_eq!(rows.len(), 5, "rows: {rows:?}");
    for row in &rows {
        assert!(!row.ok, "row: {row:?}");
        assert_eq!(row.source, "deploy-probe", "row: {row:?}");
        assert_eq!(row.latency_ms, 0, "row: {row:?}");
    }

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    let mcp = body["components"]
        .as_array()
        .expect("components array")
        .iter()
        .find(|c| c["name"] == "mcp")
        .expect("mcp component present");
    assert_eq!(mcp["state"], "failing", "mcp: {mcp}");
    assert_eq!(mcp["last_sample"]["source"], "deploy-probe");
}
