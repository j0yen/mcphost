//! loop/mcphost-fleet-signup-limit AC1 -- Given `$MCPHOST_FLEET_IPS`
//! configured with a source IP and `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR`
//! set to 1, When that IP signs up more than once within the hour, Then
//! every signup succeeds (the per-IP rate-limit gate never applies to a
//! fleet IP, and never increments its counter either). A source IP NOT in
//! that list still hits the ordinary limit at the same value, and a fleet
//! signup is still classified `synthetic`/`fleet` exactly as
//! loop/mcphost-fleet-ips already established -- only the rate-limit admit
//! check is new here.
//!
//! Prod tonight: `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` dropped from 100 to
//! 5 for the public wave; the synthetic fleet signs up hundreds of tenants
//! per hour from two or three fixed IPs already carried in
//! `$MCPHOST_FLEET_IPS` for tenant attribution (loop/mcphost-fleet-ips).

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

const FLEET_IP: &str = "203.0.113.50";
const NON_FLEET_IP: &str = "203.0.113.99";

#[tokio::test]
async fn fleet_ip_signups_all_succeed_past_a_limit_of_one() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(1, FLEET_IP).await;
    let client = McpClient::new(&server.base_url);

    for i in 0..3 {
        client
            .tools_call_with_header(
                "signup",
                json!({"name": format!("Fleet Agent {i}")}),
                ("x-forwarded-for", FLEET_IP),
            )
            .await
            .unwrap_or_else(|e| {
                panic!("fleet-ip signup {i} must bypass the 1/hour limit: {e:?}")
            });
    }

    let tenants = server.state.db.list_tenants().await.unwrap();
    assert_eq!(
        tenants.len(),
        3,
        "all three fleet-ip signups must have created a tenant, none rate limited"
    );
}

#[tokio::test]
async fn non_fleet_ip_is_still_rate_limited_at_the_same_limit() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(1, FLEET_IP).await;
    let client = McpClient::new(&server.base_url);

    client
        .tools_call_with_header(
            "signup",
            json!({"name": "Real Agent"}),
            ("x-forwarded-for", NON_FLEET_IP),
        )
        .await
        .unwrap_or_else(|e| panic!("first signup under the limit should succeed: {e:?}"));

    let err = client
        .tools_call_with_header(
            "signup",
            json!({"name": "Real Agent Two"}),
            ("x-forwarded-for", NON_FLEET_IP),
        )
        .await
        .expect_err("a non-fleet IP must still hit the 1/hour limit");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));

    let tenants = server.state.db.list_tenants().await.unwrap();
    assert_eq!(
        tenants.len(),
        1,
        "the rate-limited non-fleet signup must not create a second tenant"
    );
}

#[tokio::test]
async fn fleet_ip_signups_are_still_classified_synthetic() {
    let server = TestServer::start_with_signup_rate_limit_and_fleet_ips(1, FLEET_IP).await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call_with_header(
            "signup",
            json!({"name": "Fleet Agent"}),
            ("x-forwarded-for", FLEET_IP),
        )
        .await
        .unwrap_or_else(|e| panic!("fleet-ip signup should succeed: {e:?}"));
    let structured = extract_structured(&result);
    let namespace = structured["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .unwrap()
        .expect("tenant row exists");
    assert_eq!(
        tenant.source_class.as_deref(),
        Some("fleet"),
        "a bypassed fleet-ip signup must still be classified fleet, exactly as today"
    );
    assert_eq!(
        tenant.synthetic.as_deref(),
        Some("harness:fleet-ip"),
        "a bypassed fleet-ip signup must still be marked synthetic, exactly as today"
    );
}
