//! PRD-mcphost-incident-banner-for-agents-b
//! AC1 — Given no open incident, When `/status.json` is fetched, Then
//! `banner` is present and `null`; Given one opened via
//! `admin.incident.open`, Then `banner.id` equals it and the next
//! host-fault error carries it without a restart.

use crate::common;
use common::TestServer;
use mcphost::errors::AppError;
use serde_json::{Value, json};

async fn fetch(server: &TestServer) -> Value {
    reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse /status.json")
}

#[tokio::test]
async fn incbanb_ac01() {
    let server = TestServer::start().await;
    let base = server.state.public_url.clone();

    let before = fetch(&server).await;
    let obj = before.as_object().expect("status object");
    assert!(obj.contains_key("banner"), "banner key must always be present: {before}");
    assert!(before["banner"].is_null(), "no incident open, banner is null: {before}");

    let opened = mcphost::admin::incident_open(
        &server.state,
        &json!({"title": "storage stall", "impact": "partial", "components": ["mcp"]}),
    )
    .await
    .expect("open incident");
    let id = opened["id"].as_i64().expect("incident id");

    let after = fetch(&server).await;
    assert_eq!(after["banner"]["id"], id, "{after}");
    assert_eq!(after["banner"]["impact"], "partial");
    assert!(after["banner"]["since"].as_str().is_some_and(|s| s.contains('T')), "{after}");
    assert_eq!(after["banner"]["status_url"], format!("{base}/status.html#{id}"));

    let data = AppError::Storage("disk".into()).into_error_data_at(Some(&base)).data.expect("data");
    assert_eq!(data["incident"]["id"], id, "next host-fault error carries the incident: {data}");
}
