//! PRD-mcphost-healthz-minimal
//! AC1 — Given a running server, When `GET /healthz` arrives with no
//! `Authorization` header, Then the body is exactly `{"ok": true}` and none
//! of paying_tenants/tenants_total/tools_total/billing_mode/sandbox_*/
//! version appear.

mod common;
use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn anonymous_healthz_is_exactly_ok_true() {
    let server = TestServer::start().await;

    let resp = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    assert_eq!(
        body,
        json!({"ok": true}),
        "anonymous healthz must be exactly {{\"ok\": true}}, no other fields: {body:?}"
    );
}
