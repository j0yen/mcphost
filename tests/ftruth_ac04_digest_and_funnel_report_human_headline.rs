//! PRD-mcphost-funnel-truth
//! AC4 (P0) — Given rows of all three origins on one day, When the digest
//! (`/healthz`'s `funnel_7d`) and `admin.funnel` (this host's own
//! per-tenant funnel/progress report) run, Then the headline counts only
//! `human` rows and the per-origin breakdown lists `fleet` and `probe`
//! separately, with existing fields unchanged.

use crate::common;
use common::{ADMIN_KEY, TestServer};
use serde_json::json;

const FLEET_IP: &str = "46.225.110.44";
const HUMAN_IP: &str = "8.8.8.8";

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
async fn one_of_each_origin_reports_human_headline_and_fleet_probe_breakdown() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;

    mcphost::control::signup(
        &server.state,
        &json!({"name": "AC4 Human Tenant"}),
        HUMAN_IP,
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("human signup");
    mcphost::control::signup(
        &server.state,
        &json!({"name": "AC4 Fleet Tenant"}),
        FLEET_IP,
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("fleet signup");
    mcphost::control::signup(
        &server.state,
        &json!({"name": "AC4 Probe Tenant"}),
        FLEET_IP,
        mcphost::control::SignupAttribution {
            origin_header: Some("probe"),
            ..Default::default()
        },
    )
    .await
    .expect("probe signup");

    // `admin.funnel` ("host_progress"): existing fields unchanged --
    // `signups` still counts every row regardless of origin, same formula
    // as before this PRD.
    let report = mcphost::admin::funnel(&server.state, &json!({})).await.expect("admin.funnel");
    assert_eq!(report["signups"], json!(3), "{report:?}");
    assert_eq!(report["signups_human"], json!(1), "headline must count only human rows: {report:?}");
    assert_eq!(report["by_origin"]["human"]["signups"], json!(1), "{report:?}");
    assert_eq!(report["by_origin"]["fleet"]["signups"], json!(1), "{report:?}");
    assert_eq!(report["by_origin"]["probe"]["signups"], json!(1), "{report:?}");

    // `/healthz`'s `funnel_7d` (the digest's own data source): the
    // pre-existing `source_class = external`-only `signups` field is
    // unchanged (fleet-IP signups, whether plain fleet or probe, were
    // already excluded from it before this PRD; only the human tenant
    // counts) -- the new `signups_human`/`by_origin` ride alongside it.
    let health = healthz(&server.base_url).await;
    let funnel_7d = &health["funnel_7d"];
    assert_eq!(funnel_7d["signups"], json!(1), "existing external-only field must be unchanged: {funnel_7d:?}");
    assert_eq!(funnel_7d["signups_human"], json!(1), "{funnel_7d:?}");
    assert_eq!(funnel_7d["by_origin"]["human"]["signups"], json!(1), "{funnel_7d:?}");
    assert_eq!(funnel_7d["by_origin"]["fleet"]["signups"], json!(1), "{funnel_7d:?}");
    assert_eq!(funnel_7d["by_origin"]["probe"]["signups"], json!(1), "{funnel_7d:?}");
}
