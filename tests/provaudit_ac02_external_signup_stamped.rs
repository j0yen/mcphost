//! PRD-mcphost-provenance-audit
//! AC2 — Given a signup from a public IP with no synthetic marker, When it
//! lands, Then `origin = external` and `/healthz`'s `tenants.external`
//! count increments by exactly 1.

mod common;
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
async fn external_signup_stamps_origin_external_and_increments_healthz_external() {
    let server = common::TestServer::start().await;

    let before = healthz(&server.base_url).await;
    let external_before = before["tenants"]["external"].as_i64().unwrap();

    // Same pattern as `attrib_ac4`: a real TCP connection to this test
    // server is always loopback, so a genuinely non-loopback, unmarked
    // signup is exercised via `control::signup` directly.
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "Real External Signup"}),
        "203.0.113.9",
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
    assert_eq!(tenant.origin, "external");

    let after = healthz(&server.base_url).await;
    let external_after = after["tenants"]["external"].as_i64().unwrap();
    assert_eq!(
        external_after,
        external_before + 1,
        "external count must move by exactly 1: before={before:?} after={after:?}"
    );
}
