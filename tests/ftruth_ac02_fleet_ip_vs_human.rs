//! PRD-mcphost-funnel-truth
//! AC2 (P0) — Given a signup from an IP in the fleet list with no header,
//! When the tenant is created, Then `origin = 'fleet'`; and from an IP
//! outside the list, `origin = 'human'`.

use crate::common;
use common::TestServer;
use serde_json::json;

const FLEET_IP: &str = "46.225.110.44";
const NON_FLEET_IP: &str = "8.8.8.8";

#[tokio::test]
async fn fleet_ip_with_no_header_is_fleet_and_outside_ip_is_human() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(100, FLEET_IP).await;

    let fleet_result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Fleet Tenant"}),
        FLEET_IP,
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("fleet signup");
    let fleet_ns = fleet_result["tenant"].as_str().expect("tenant").to_string();
    let fleet_tenant = server
        .state
        .db
        .find_tenant_by_namespace(fleet_ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(fleet_tenant.funnel_origin, "fleet", "{fleet_tenant:?}");

    let human_result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC2 Human Tenant"}),
        NON_FLEET_IP,
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("human signup");
    let human_ns = human_result["tenant"].as_str().expect("tenant").to_string();
    let human_tenant = server
        .state
        .db
        .find_tenant_by_namespace(human_ns)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(human_tenant.funnel_origin, "human", "{human_tenant:?}");
}
