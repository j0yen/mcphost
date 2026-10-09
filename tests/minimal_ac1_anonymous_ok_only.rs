//! PRD-mcphost-healthz-minimal
//! AC1 — Given a running server, When `GET /healthz` arrives with no
//! `Authorization` header, Then the body is exactly the keys `ok`, `version`,
//! `git_sha` (PRD-mcphost-healthz-version-field R5) and none of
//! paying_tenants/tenants_total/tools_total/billing_mode/sandbox_* appear.

use crate::common;
use common::TestServer;
use mcphost::build_info::BUILD;
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
        json!({"ok": true, "version": BUILD.version, "git_sha": BUILD.git_sha}),
        "anonymous healthz must be exactly ok/version/git_sha, no other fields: {body:?}"
    );
}
