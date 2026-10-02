//! PRD-mcphost-chain-run-lineage AC12 (P1) — Given two chain runs and one
//! `compose_input_missing` refusal, When `/healthz` is read, Then
//! `runs.composition_children_total == 6` and
//! `runs.composition_parents_failed_input_total == 1`.

use crate::common;
use common::{chain_kind_registry, publish, signup, ADMIN_KEY, TestServer};
use serde_json::json;

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_reports_composition_children_and_failed_input_totals() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC12 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let schema = json!({"type": "object"});
    publish(&client, "fetch_data", "echo", json!({"schema": schema})).await;
    publish(&client, "transform", "echo", json!({"schema": schema})).await;
    publish(&client, "write", "echo", json!({"schema": schema})).await;

    let chain_spec = json!({
        "steps": [
            {"tool": "fetch_data", "args": {"url": "$.input.url"}},
            {"tool": "transform", "args": {"rows": "$.prev.result.url"}},
            {"tool": "write", "args": {"rows": "$.prev.result.rows", "region": "$.input.region"}},
        ]
    });
    let chain = publish(&client, "daily_pipeline", "chain", chain_spec).await;
    assert_eq!(chain, format!("{ns}.daily_pipeline"));

    // Two full runs -- 3 children each, 6 total.
    for _ in 0..2 {
        client
            .tools_call(&chain, json!({"url": "https://example.com/data", "region": "eu"}))
            .await
            .expect("call ok");
    }
    // One refusal before step 1 -- zero more children, one failed-input parent.
    client
        .tools_call(&chain, json!({}))
        .await
        .expect_err("missing inputs must refuse");

    let body = healthz(&server.base_url).await;
    assert_eq!(body["runs"]["composition_children_total"], json!(6), "{body}");
    assert_eq!(body["runs"]["composition_parents_failed_input_total"], json!(1), "{body}");
}
