//! PRD-mcphost-session-bound-tenant-key
//! AC2 (P0) — Given that same connection, When a second `host.tool_publish`
//! arrives without `tenant_key`, Then no tenant is created, the response is
//! `tenant_key_missing` with `data.tenant` equal to the first tenant's
//! namespace and the hint naming it, and `signup_events` holds one
//! `implicit_second_signup_blocked` row.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn second_bare_publish_on_the_same_connection_is_refused_and_named() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let first = session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("the first bare host.tool_publish call must succeed");
    let first = extract_structured(&first);
    let tenant_ns = first["onboarding"]["tenant"]
        .as_str()
        .expect("onboarding.tenant present")
        .to_string();

    let before = server.state.db.count_implicit_second_signup_blocked(0).await.expect("count");
    assert_eq!(before, 0, "no blocked second signup has happened yet");

    let second = session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "second",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await;
    let err = second.expect_err("a second bare host.tool_publish on the same connection must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(
        err.data.get("tenant").and_then(serde_json::Value::as_str),
        Some(tenant_ns.as_str()),
        "data.tenant must name the first tenant: {err:?}"
    );
    let hint = err.data.get("hint").and_then(serde_json::Value::as_str).unwrap_or_default();
    assert!(
        hint.contains(&tenant_ns),
        "the hint must name the tenant too: {hint:?}"
    );

    // No second tenant was created.
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "no second tenant must have been created");
    assert_eq!(tenants[0].namespace, tenant_ns);

    // signup_events holds exactly one implicit_second_signup_blocked row.
    let after = server.state.db.count_implicit_second_signup_blocked(0).await.expect("count");
    assert_eq!(after, 1, "signup_events must hold exactly one blocked-second-signup row");
}
