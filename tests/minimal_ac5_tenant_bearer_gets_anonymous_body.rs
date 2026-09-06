//! PRD-mcphost-healthz-minimal
//! AC5 (P1) — Given a valid tenant key, When it is presented on `/healthz`,
//! Then the anonymous body is returned (tenant keys are not admin).

mod common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_bearer_does_not_unlock_admin_diagnostics() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Not Admin").await;

    let resp = reqwest::Client::new()
        .get(format!("{}/healthz", server.base_url))
        .bearer_auth(&key)
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = resp.json().await.expect("parse /healthz");
    assert_eq!(
        body,
        json!({"ok": true}),
        "a valid tenant key must still get the anonymous body, not the admin document: {body:?}"
    );
}
