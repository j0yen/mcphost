//! PRD-mcphost-external-class-positive-evidence
//! AC1 — Given a signup from a non-loopback, non-fleet IP with no
//! `X-Mcphost-Synthetic` header, When the tenant row is written, Then
//! `origin='unattributed'`, `origin_detail='no-positive-evidence'` and
//! `tenants_by_origin.unattributed` in healthz rises by one.

use crate::common;
use common::ADMIN_KEY;
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
async fn mcphost_external_class_positive_evidence_ac01_non_fleet_signup_is_unattributed() {
    let server = common::TestServer::start().await;

    let before = healthz(&server.base_url).await;
    assert_eq!(before["tenants_by_origin"]["unattributed"], json!(0), "{before:?}");

    let out = mcphost::control::signup(
        &server.state,
        &json!({"name": "Stranger Co"}),
        "203.0.113.77",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("non-fleet signup");
    let namespace = out["namespace"].as_str().expect("namespace").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("lookup")
        .expect("tenant row");
    assert_eq!(tenant.origin, "unattributed");
    assert_eq!(tenant.origin_detail.as_deref(), Some("no-positive-evidence"));

    let after = healthz(&server.base_url).await;
    assert_eq!(after["tenants_by_origin"]["unattributed"], json!(1), "{after:?}");
    assert_eq!(after["tenants_by_origin"]["external"], json!(0), "{after:?}");
    assert_eq!(after["tenants_real"], json!(0), "{after:?}");
    assert_eq!(after["origin_unknown"], json!(0), "{after:?}");
}
