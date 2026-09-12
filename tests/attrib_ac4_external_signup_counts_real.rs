//! PRD-mcphost-tenant-attribution
//! AC4 — Given a signup from a non-loopback IP with no stamp and an
//! unknown client, When it completes, Then `source_class = external` and
//! `tenants_real` increments by one.
//!
//! This suite's `TestServer` is a real TCP listener, so an actual
//! connection to it is always `127.0.0.1` -- there is no way to make a
//! genuinely non-loopback HTTP request against it. `control::signup`
//! itself takes `source_ip: &str` as a plain argument (the HTTP layer's
//! own extraction, `handler::source_ip`, is a thin wrapper around axum's
//! `ConnectInfo`), so this test calls it directly with a fake external IP
//! -- exactly the seam the PRD's non-goals ("no change to signup
//! semantics") describe, exercised at the business-logic layer rather
//! than faked at the transport layer.

use crate::common;
use common::{ADMIN_KEY, TestServer};
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
async fn external_signup_classifies_real_and_increments_tenants_real() {
    let server = TestServer::start().await;

    let before = healthz(&server.base_url).await;
    let real_before = before["tenants"]["external"].as_i64().unwrap();

    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "Joe's Real Company"}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("external signup");
    let ns = result["tenant"].as_str().expect("tenant field").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.source_class.as_deref(), Some("external"));
    assert_eq!(
        tenant.synthetic, None,
        "an external tenant must never carry a synthetic label"
    );

    let after = healthz(&server.base_url).await;
    let real_after = after["tenants"]["external"].as_i64().unwrap();
    assert_eq!(real_after, real_before + 1, "{after:?}");
}
