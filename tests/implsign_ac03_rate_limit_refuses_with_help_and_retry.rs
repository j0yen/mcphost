//! PRD-mcphost-implicit-signup
//! AC3 (P0) — Given one IP that has created the limiter's maximum tenants
//! this hour, When an anonymous `host.*` call arrives, Then the error class
//! is `signup_rate_limited`, `data.help` ends in `/u/new`, `data.retry_after_s`
//! is present, and no tenant or tool was created.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn over_the_limit_anonymous_call_is_signup_rate_limited() {
    // A one-tenant-per-hour cap so the very next anonymous call is over it.
    let server = TestServer::start_with_signup_rate_limit(1).await;

    // Consume the one implicit-signup slot this source IP gets.
    let first_session = McpClient::new(&server.base_url).with_session_continuity();
    let first = first_session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("the first implicit signup must succeed under a 1-per-hour cap");
    assert!(extract_structured(&first).get("onboarding").is_some());

    let before = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(before.len(), 1);

    // A second, independent session (same loopback source IP, no prior
    // binding) tries a bare host.* call and must be refused.
    let second_session = McpClient::new(&server.base_url).with_session_continuity();
    let err = second_session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "world",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect_err("an anonymous call past the per-IP cap must be refused");

    assert_eq!(err.error_code.as_deref(), Some("signup_rate_limited"), "{err:?}");
    let help = err.data["help"].as_str().expect("data.help must be present");
    assert!(help.ends_with("/u/new"), "data.help must end in /u/new, got {help}");
    assert!(
        err.data["retry_after_s"].is_i64() || err.data["retry_after_s"].is_u64(),
        "data.retry_after_s must be present, got {err:?}"
    );

    // No tenant or tool was created by the refused call.
    let after = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(after.len(), 1, "the rate-limited call must not create a tenant");
    assert_eq!(after[0].id, before[0].id);
    let tools = server.state.db.list_tools(before[0].id).await.expect("list tools");
    assert_eq!(
        tools.len(),
        1,
        "only the first call's own tool ('hello') must exist; the refused call's 'world' must not"
    );
    assert_eq!(tools[0].name, "hello");
}
