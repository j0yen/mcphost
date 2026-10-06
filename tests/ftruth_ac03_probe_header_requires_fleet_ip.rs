//! PRD-mcphost-funnel-truth
//! AC3 (P0) — Given a signup with `X-Mcphost-Origin: probe` from a fleet
//! IP, When created, Then `origin = 'probe'`; the same header from a
//! non-fleet IP yields `origin = 'human'`.

use crate::common;
use common::TestServer;
use serde_json::json;

const FLEET_IP: &str = "46.225.110.44";
const NON_FLEET_IP: &str = "8.8.8.8";

#[tokio::test]
async fn probe_header_from_fleet_ip_is_probe_same_header_outside_fleet_is_human() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;

    let probe_result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC3 Probe Tenant"}),
        FLEET_IP,
        mcphost::control::SignupAttribution {
            origin_header: Some("probe"),
            ..Default::default()
        },
    )
    .await
    .expect("probe signup");
    let probe_ns = probe_result["tenant"].as_str().expect("tenant").to_string();
    let probe_tenant = server
        .state
        .db
        .find_tenant_by_namespace(probe_ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(probe_tenant.funnel_origin, "probe", "{probe_tenant:?}");

    // requirement 2's own "a stranger cannot hide as a probe": the same
    // header from a non-fleet IP must NOT grant probe status.
    let stranger_result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC3 Stranger Tenant"}),
        NON_FLEET_IP,
        mcphost::control::SignupAttribution {
            origin_header: Some("probe"),
            ..Default::default()
        },
    )
    .await
    .expect("stranger signup");
    let stranger_ns = stranger_result["tenant"].as_str().expect("tenant").to_string();
    let stranger_tenant = server
        .state
        .db
        .find_tenant_by_namespace(stranger_ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(
        stranger_tenant.funnel_origin, "human",
        "a probe header from outside the fleet list must not be honoured: {stranger_tenant:?}"
    );
}
