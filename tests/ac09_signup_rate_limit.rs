//! AC9 — Given a source IP that has signed up 5 times within an hour, When
//! it calls `signup` a sixth time, Then the call returns error
//! `rate_limited` and no tenant is created.

mod common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn sixth_signup_in_an_hour_is_rate_limited() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    for i in 0..5 {
        client
            .tools_call("signup", json!({"name": format!("Agent {i}")}))
            .await
            .unwrap_or_else(|e| panic!("signup {i} should succeed: {e:?}"));
    }
    let before = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(before, 5);

    let err = client
        .tools_call("signup", json!({"name": "Agent 6"}))
        .await
        .expect_err("6th signup within the hour must be rate limited");
    assert_eq!(err.error_code.as_deref(), Some("rate_limited"));

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(after, 5, "the rate-limited signup must not create a tenant");
}
