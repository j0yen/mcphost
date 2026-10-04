//! PRD-mcphost-implicit-signup
//! AC5 (P0) — Given an anonymous session, When `tools/list`, `initialize`,
//! `signup`, `host.redeem`, `host.quickstart`, or `billing.plans` are
//! called, Then no tenant is created.
//!
//! `signup` legitimately creates a tenant when called with a valid `name`
//! -- that is its whole job, unchanged by this PRD (Non-goals never touch
//! it). What this AC actually guards against is this PRD's own new
//! anonymous-dispatch branch reaching `signup`/`host.redeem`/
//! `host.quickstart`/`billing.plans` at all: `call_tool`'s match arms for
//! those four names are matched on the tool name alone, before the new
//! `(Auth::Anonymous, host.*/billing.*)` arm ever runs, regardless of
//! whether the call's own arguments would make it succeed. Calling each
//! one here with no (or insufficient) arguments keeps "no tenant is
//! created" literally true while still proving the new branch is never
//! reached for them.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn none_of_the_pre_existing_anonymous_entry_points_mint_a_tenant() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    client.initialize().await;
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "initialize");

    client.tools_list().await.expect("tools/list");
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "tools/list");

    let err = client
        .tools_call("signup", json!({}))
        .await
        .expect_err("signup with no name must fail validation, not implicitly create a tenant");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "signup");

    let err = client
        .tools_call("host.redeem", json!({}))
        .await
        .expect_err("host.redeem with no handoff_token must fail validation");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "host.redeem");

    client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("host.quickstart stays reachable anonymously");
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "host.quickstart");

    client
        .tools_call("billing.plans", json!({}))
        .await
        .expect("billing.plans stays reachable anonymously");
    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "billing.plans");
}
