//! PRD-mcphost-healthz-minimal
//! AC2 — Given the configured admin key, When `GET /healthz` carries it as
//! a bearer, Then the response contains the full diagnostics document with
//! the same fields the current release publishes.

mod common;
use common::{ADMIN_KEY, TestServer};

#[tokio::test]
async fn admin_bearer_unlocks_full_diagnostics_document() {
    let server = TestServer::start().await;

    let resp = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(ADMIN_KEY)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    for field in [
        "version",
        "db_ok",
        "tools_total",
        "tenants_total",
        "tenants_probe",
        "sandbox_mechanism",
        "billing_mode",
        "paying_tenants",
    ] {
        assert!(
            body.get(field).is_some(),
            "admin healthz document must still carry `{field}`: {body:?}"
        );
    }
    assert!(
        body.get("ok").is_none(),
        "the full document has no `ok` key of its own: {body:?}"
    );
}
