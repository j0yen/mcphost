//! PRD-mcphost-external-class-positive-evidence
//! AC1 (P0) — Given a signup from a non-loopback, non-fleet IP with no
//! `X-Mcphost-Synthetic` header, When the tenant row is written, Then
//! `origin='unattributed'` and `origin_detail='no-positive-evidence'` and
//! `tenants_by_origin.unattributed` in healthz rises by one.
//!
//! A real TCP connection to the test server is always loopback, so the
//! non-loopback signup goes through `control::signup` with an explicit peer
//! IP (same seam `provaudit_ac02` uses).

use crate::common;
use common::ADMIN_KEY;
use mcphost::control::{SignupAttribution, signup};
use serde_json::{Value, json};

async fn healthz(base_url: &str) -> Value {
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
async fn mcphost_external_class_positive_evidence_ac01_signup_lands_unattributed() {
    let server = common::TestServer::start().await;
    let before = healthz(&server.base_url).await;
    let by_origin_before = &before["tenants_by_origin"];
    let unattributed_before = by_origin_before["unattributed"].as_i64().expect("unattributed key");
    let external_before = by_origin_before["external"].as_i64().expect("external key");

    let result = signup(
        &server.state,
        &json!({"name": "Unknown Agent Co"}),
        "203.0.113.77",
        SignupAttribution::default(),
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
        after["tenants_by_origin"]["unattributed"].as_i64(),
        Some(unattributed_before + 1),
        "{after:?}"
    );
    assert_eq!(
        after["tenants_by_origin"]["external"].as_i64(),
        Some(external_before),
        "an unattributed signup must never raise external: {after:?}"
    );
    assert_eq!(after["origin_unknown"], json!(0), "{after:?}");
}

#[tokio::test]
async fn mcphost_external_class_positive_evidence_ac01_signup_event_is_unattributed_too() {
    let server = common::TestServer::start().await;
    signup(
        &server.state,
        &json!({"name": "Unknown Agent Two"}),
        "203.0.113.78",
        SignupAttribution::default(),
    )
    .await
    .expect("signup");
    let health = healthz(&server.base_url).await;
    assert_eq!(health["signups"]["unattributed"], json!(1), "{health:?}");
    assert_eq!(health["signups"]["external"], json!(0), "{health:?}");
}
