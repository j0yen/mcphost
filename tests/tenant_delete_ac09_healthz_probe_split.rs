//! PRD-mcphost-tenant-delete
//! AC9 — Given probe tenants and one real tenant, When `/healthz` is
//! fetched, Then it reports `tenants_probe` and `tenants_total` such that
//! `tenants_total - tenants_probe` equals the real count.

mod common;
use common::{TestServer, signup};

async fn healthz(base_url: &str) -> serde_json::Value {
    reqwest::get(format!("{base_url}/healthz"))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz")
}

#[tokio::test]
async fn healthz_splits_probe_tenants_from_the_total() {
    let server = TestServer::start().await;

    let before = healthz(&server.base_url).await;
    let probe_before = before["tenants_probe"].as_i64().unwrap();
    let total_before = before["tenants_total"].as_i64().unwrap();

    signup(&server.base_url, "panel_rapid_prototyper_01").await;
    signup(&server.base_url, "probe-smoke-test").await;
    signup(&server.base_url, "Joe's Real Company").await;

    let after = healthz(&server.base_url).await;
    let probe_after = after["tenants_probe"].as_i64().unwrap();
    let total_after = after["tenants_total"].as_i64().unwrap();

    assert_eq!(total_after, total_before + 3);
    assert_eq!(
        probe_after,
        probe_before + 2,
        "only the panel_ and probe- tenants count as probes"
    );
    assert_eq!(
        total_after - probe_after,
        (total_before - probe_before) + 1,
        "tenants_total - tenants_probe must equal the real-tenant count"
    );
}
