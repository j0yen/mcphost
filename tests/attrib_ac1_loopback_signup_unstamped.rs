//! PRD-mcphost-tenant-attribution
//! AC1 — Given a signup from 127.0.0.1 with no stamp, When it completes,
//! Then the tenant has `source_class = loopback` and `synthetic =
//! harness:unstamped`, and `/healthz tenants_real` does not count it.

mod common;
use common::{ADMIN_KEY, TestServer, signup};

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
async fn loopback_signup_defaults_to_harness_unstamped_and_is_not_real() {
    let server = TestServer::start().await;

    let before = healthz(&server.base_url).await;
    let real_before = before["tenants_real"].as_i64().unwrap();

    let (ns, _) = signup(&server.base_url, "No Stamp Agent").await;

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(tenant.source_class.as_deref(), Some("loopback"));
    assert_eq!(tenant.synthetic.as_deref(), Some("harness:unstamped"));

    let after = healthz(&server.base_url).await;
    let real_after = after["tenants_real"].as_i64().unwrap();
    assert_eq!(
        real_after, real_before,
        "a loopback signup must not move tenants_real: {after:?}"
    );
    // The bug this PRD fixes: the old definition (`synthetic IS NULL`)
    // would have miscounted this same tenant as real. Assert the total
    // moved (so this isn't a vacuous "nothing happened" pass) but real
    // didn't.
    let total_after = after["tenants_total"].as_i64().unwrap();
    assert_eq!(total_after, 1, "{after:?}");
    assert_eq!(real_after, 0, "{after:?}");
}
