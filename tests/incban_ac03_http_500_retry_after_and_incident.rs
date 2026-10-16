//! PRD-mcphost-incident-banner-for-agents
//! AC3 — Given an open incident and an HTTP route failing with 500, When
//! the response is built, Then it carries `Retry-After: 30` and the JSON
//! body's `data.incident.id` equals the open incident id.

use crate::common;
use common::TestServer;
use mcphost::errors::AppError;
use serde_json::json;

#[tokio::test]
async fn incban_ac03() {
    let server = TestServer::start().await;
    let state = &server.state;

    // Control: no incident open, a 500 carries neither.
    let before = mcphost::public_tool::app_error_response(state, AppError::Storage("disk".into()));
    assert_eq!(before.status(), 500);
    assert!(before.headers().get("retry-after").is_none());

    let opened = mcphost::admin::incident_open(
        state,
        &json!({"title": "storage stall", "impact": "major", "components": ["mcp"]}),
    )
    .await
    .expect("open incident");
    let id = opened["id"].as_i64().expect("incident id");

    let response = mcphost::public_tool::app_error_response(state, AppError::Storage("disk".into()));
    assert_eq!(response.status(), 500);
    assert_eq!(response.headers().get("retry-after").expect("Retry-After header"), "30");
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    assert_eq!(body["data"]["incident"]["id"], id, "{body}");
    assert_eq!(body["data"]["incident"]["retry_after_s"], 30);
}
