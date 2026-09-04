//! PRD-mcphost-session-key
//! AC10 — Given a valid tenant key, When it is passed as `tenant_key` to an
//! `admin.*` tool, Then the call is forbidden and no admin action is
//! performed.

mod common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tenant_key_never_reaches_an_admin_tool() {
    let server = TestServer::start().await;
    let (victim_ns, _victim_key) = signup(&server.base_url, "Victim").await;
    let (_attacker_ns, attacker_key) = signup(&server.base_url, "Attacker").await;

    let anon = McpClient::new(&server.base_url);
    let err = anon
        .tools_call(
            "admin.tenant_disable",
            json!({"tenant": victim_ns, "tenant_key": attacker_key}),
        )
        .await
        .expect_err("a tenant_key must never authorize an admin.* tool");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"));

    // No admin action was performed: the victim tenant is still enabled.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let tenants = extract_structured(
        &admin
            .tools_call("admin.tenants", json!({}))
            .await
            .expect("admin.tenants"),
    );
    let row = tenants["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(victim_ns))
        .unwrap_or_else(|| panic!("victim tenant missing from admin.tenants: {tenants}"));
    assert_eq!(
        row["disabled"],
        json!(false),
        "the victim tenant must not have been disabled: {row}"
    );
}
