//! AC5 — Given the live deployment, When an operator queries `tenants` for
//! a self-offboarded row, Then it is distinguishable from an admin-disabled
//! row (reason field populated).

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

fn tenant_row<'a>(tenants: &'a serde_json::Value, ns: &str) -> &'a serde_json::Value {
    tenants["tenants"]
        .as_array()
        .expect("tenants array")
        .iter()
        .find(|t| t["tenant"] == json!(ns))
        .unwrap_or_else(|| panic!("tenant {ns} not found in admin.tenants listing"))
}

#[tokio::test]
async fn admin_tenants_shows_disabled_reason_distinguishing_self_offboard_from_admin_disable() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // One tenant closes its own account through the public path...
    let (self_ns, self_key) = signup(&server.base_url, "Self Offboarder").await;
    let self_client = McpClient::with_bearer(&server.base_url, &self_key);
    self_client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("self_offboard must succeed");

    // ...another is disabled by an operator instead.
    let (admin_disabled_ns, _admin_disabled_key) = signup(&server.base_url, "Admin Disabled").await;
    admin
        .tools_call(
            "admin.tenant_disable",
            json!({"tenant": admin_disabled_ns}),
        )
        .await
        .expect("admin.tenant_disable must succeed");

    // A third, still-enabled tenant, so the "None"/enabled case is pinned
    // alongside the two disabled ones.
    let (enabled_ns, _enabled_key) = signup(&server.base_url, "Still Enabled").await;

    let listing = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants must succeed");
    let structured = common::extract_structured(&listing);

    let self_row = tenant_row(&structured, &self_ns);
    assert_eq!(self_row["disabled"], json!(true));
    assert_eq!(self_row["disabled_reason"], json!("self_offboard"));

    let admin_row = tenant_row(&structured, &admin_disabled_ns);
    assert_eq!(admin_row["disabled"], json!(true));
    assert_eq!(admin_row["disabled_reason"], json!("admin_disable"));

    let enabled_row = tenant_row(&structured, &enabled_ns);
    assert_eq!(enabled_row["disabled"], json!(false));
    assert_eq!(enabled_row["disabled_reason"], json!(null));

    assert_ne!(self_row["disabled_reason"], admin_row["disabled_reason"]);
}
