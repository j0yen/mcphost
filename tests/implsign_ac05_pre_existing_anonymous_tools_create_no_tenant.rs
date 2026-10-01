//! PRD-mcphost-implicit-signup
//! AC5 (P0) — Given an anonymous session, When `tools/list`, `initialize`,
//! `signup`, `host.redeem`, `host.quickstart`, or `billing.plans` are
//! called, Then no tenant is created.
//!
//! `signup` itself always creates exactly one (explicit) tenant -- that is
//! its whole purpose, unchanged by this PRD. What this AC actually pins
//! down is that none of the other five calls, and not even `signup`
//! itself, ever creates a *second*, implicit tenant underneath -- the new
//! `Auth::Anonymous` dispatch arm this PRD adds is never reached for any
//! of these six names, because each is matched by its own, earlier arm
//! regardless of `auth` (see `call_tool`'s match).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn pre_existing_anonymous_surface_mints_no_implicit_tenant() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let before = server.state.db.list_tenants().await.unwrap().len();

    // `initialize` + `tools/list`: every `tools_call` already performs its
    // own `initialize` underneath (stateless-request mode), so this covers
    // both wire methods the AC names.
    client
        .tools_list()
        .await
        .expect("tools/list must stay anonymous-reachable");

    client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("host.quickstart must stay anonymous-reachable");

    client
        .tools_call("billing.plans", json!({}))
        .await
        .expect("billing.plans must stay anonymous-reachable");

    // An invalid handoff token still reaches `control::redeem` -- it just
    // fails there, never anywhere near implicit signup.
    let _ = client
        .tools_call("host.redeem", json!({"handoff_token": "not-a-real-token"}))
        .await;

    let after_no_signup = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after_no_signup, before,
        "tools/list, host.quickstart, billing.plans and host.redeem must create no tenant"
    );

    // `signup` itself still creates exactly one tenant -- proving the new
    // implicit-signup arm never double-fires underneath an explicit
    // signup call.
    client
        .tools_call("signup", json!({"name": "AC5 Explicit"}))
        .await
        .expect("signup");
    let after_signup = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after_signup,
        before + 1,
        "signup must create exactly one tenant, never two"
    );
}
