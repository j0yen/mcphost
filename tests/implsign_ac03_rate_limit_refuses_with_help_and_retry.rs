//! PRD-mcphost-implicit-signup
//! AC3 (P0) — Given one IP that has created the limiter's maximum tenants
//! this hour, When an anonymous `host.*` call arrives, Then the error
//! class is `signup_rate_limited`, `data.help` ends in `/u/new`,
//! `data.retry_after_s` is present, and no tenant or tool was created.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn sixth_implicit_signup_from_the_same_ip_is_rate_limited() {
    let limit = 5;
    let server = TestServer::start_with_signup_rate_limit(limit).await;

    // Exhaust the limiter the same way AC4 of url-bound-tenants does:
    // `limit` independent anonymous bare calls, each minting its own
    // implicit tenant from the same test-harness IP.
    for i in 0..limit {
        let client = McpClient::new(&server.base_url);
        client
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("implicit signup {i} should succeed: {e:?}"));
    }

    let before_tenants = server.state.db.list_tenants().await.unwrap().len();

    let one_too_many = McpClient::new(&server.base_url);
    let err = one_too_many
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect_err("past the limiter's cap, implicit signup must refuse, not create a tenant");

    assert_eq!(
        err.error_code.as_deref(),
        Some("signup_rate_limited"),
        "{err:?}"
    );
    let help = err.data["help"]
        .as_str()
        .unwrap_or_else(|| panic!("data.help must be present: {err:?}"));
    assert!(
        help.ends_with("/u/new"),
        "data.help must point at /u/new: {help}"
    );
    assert!(
        err.data["retry_after_s"].is_i64() || err.data["retry_after_s"].is_u64(),
        "data.retry_after_s must be present: {err:?}"
    );

    let after_tenants = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after_tenants, before_tenants,
        "no tenant must be created by the refused call"
    );
}
