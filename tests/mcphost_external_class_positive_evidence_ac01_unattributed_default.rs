//! PRD-mcphost-external-class-positive-evidence
//! AC1 — Given a signup from a non-loopback, non-fleet IP with no
//! `X-Mcphost-Synthetic` header, When the tenant row is written, Then
//! `origin='unattributed'`, `origin_detail='no-positive-evidence'` and
//! `tenants_by_origin.unattributed` in healthz rises by one.

use crate::common;
use common::ADMIN_KEY;
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
async fn mcphost_external_class_positive_evidence_ac01() {
    let server = common::TestServer::start().await;
    let before = healthz(&server.base_url).await;
    let unattributed_before = before["tenants_by_origin"]["unattributed"].as_i64().unwrap();
    let external_before = before["tenants_by_origin"]["external"].as_i64().unwrap();

    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "No Evidence Signup"}),
        "203.0.113.9",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let ns = result["tenant"].as_str().expect("tenant field").to_string();
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.origin, "unattributed");
    assert_eq!(tenant.origin_detail.as_deref(), Some("no-positive-evidence"));

    let after = healthz(&server.base_url).await;
    assert_eq!(
        after["tenants_by_origin"]["unattributed"].as_i64().unwrap(),
        unattributed_before + 1,
        "unattributed must rise by exactly 1: {after:?}"
    );
    assert_eq!(
        after["tenants_by_origin"]["external"].as_i64().unwrap(),
        external_before,
        "external must not move on a header-less signup"
    );
    assert_eq!(after["tenants_real"], after["tenants_by_origin"]["external"]);
}
