//! AC9 — Given a source IP that has signed up 5 times within an hour, When
//! it calls `signup` a sixth time, Then the call returns error
//! `rate_limited` and no tenant is created.
//!
//! PRD-mcphost-signup-rate-configurable: the cap now lives on
//! `AppState::signup_rate_limit_per_hour` (read from
//! `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` at real startup, defaulting to
//! [`mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR`]) rather than a bare
//! compile-time const, so requirement 3 / AC1 below reads the limit off
//! `server.state` instead of hardcoding `5`. AC2's override path is
//! covered by `hundredth_signup_succeeds_with_env_raised_limit` below.

mod common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn sixth_signup_in_an_hour_is_rate_limited() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let limit = server.state.signup_rate_limit_per_hour;
    assert_eq!(
        limit,
        mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        "default (env var unset) must match the documented fallback"
    );

    for i in 0..limit {
        client
            .tools_call("signup", json!({"name": format!("Agent {i}")}))
            .await
            .unwrap_or_else(|e| panic!("signup {i} should succeed: {e:?}"));
    }
    let before = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(before, limit as usize);

    let err = client
        .tools_call("signup", json!({"name": "Agent one too many"}))
        .await
        .expect_err("signup past the limit within the hour must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after, limit as usize,
        "the rate-limited signup must not create a tenant"
    );
}

/// AC2 (PRD-mcphost-signup-rate-configurable): with the effective limit
/// raised to 100, all 100 signups from one source succeed and the 101st is
/// still limited -- proving the gate reads `state.signup_rate_limit_per_hour`
/// rather than the old hardcoded 5, at the same limit the PRD's example
/// names.
#[tokio::test]
async fn hundredth_signup_succeeds_with_env_raised_limit() {
    let server = TestServer::start_with_signup_rate_limit(100).await;
    let client = McpClient::new(&server.base_url);

    for i in 0..100 {
        client
            .tools_call("signup", json!({"name": format!("Agent {i}")}))
            .await
            .unwrap_or_else(|e| panic!("signup {i} should succeed under a 100 cap: {e:?}"));
    }
    let before = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(before, 100);

    let err = client
        .tools_call("signup", json!({"name": "Agent 101"}))
        .await
        .expect_err("the 101st signup within the hour must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(after, 100, "the rate-limited signup must not create a tenant");
}

/// P1 requirement 6: a small override (2) limits the 3rd signup, alongside
/// the default-path test above.
#[tokio::test]
async fn third_signup_is_limited_with_small_override() {
    let server = TestServer::start_with_signup_rate_limit(2).await;
    let client = McpClient::new(&server.base_url);

    for i in 0..2 {
        client
            .tools_call("signup", json!({"name": format!("Agent {i}")}))
            .await
            .unwrap_or_else(|e| panic!("signup {i} should succeed under a 2 cap: {e:?}"));
    }

    let err = client
        .tools_call("signup", json!({"name": "Agent 3"}))
        .await
        .expect_err("the 3rd signup within the hour must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(after, 2, "the rate-limited signup must not create a tenant");
}
