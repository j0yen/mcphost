//! PRD-mcphost-chart-in-a-minute
//! AC10 — Given 100 stored charts, When a 101st is shared, Then the oldest
//! is gone and `host.table.charts` lists 100.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn hundred_and_first_share_evicts_the_oldest() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Chart AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "ticks", "columns": {"n": "integer"}}))
        .await
        .expect("create ticks");
    client
        .tools_call("host.table.append", json!({"table": "ticks", "rows": [{"n": 1}]}))
        .await
        .expect("append ticks");

    let mut chart_ids = Vec::with_capacity(101);
    for _ in 0..101 {
        let result = client
            .tools_call("host.table.chart", json!({"sql": "SELECT COUNT(*) FROM ticks", "share": true}))
            .await
            .expect("host.table.chart share");
        let chart = extract_structured(&result);
        chart_ids.push(chart["chart_id"].as_str().expect("chart_id is a string").to_string());
    }
    let first_id = &chart_ids[0];
    let last_id = &chart_ids[100];

    let listing = client.tools_call("host.table.charts", json!({})).await.expect("host.table.charts");
    let charts = extract_structured(&listing);
    let listed = charts["charts"].as_array().expect("charts array");
    assert_eq!(listed.len(), 100, "charts: {charts}");

    let listed_ids: Vec<&str> = listed.iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(!listed_ids.contains(&first_id.as_str()), "the oldest chart must be evicted: {listed_ids:?}");
    assert!(listed_ids.contains(&last_id.as_str()), "the newest chart must still be listed: {listed_ids:?}");
}
