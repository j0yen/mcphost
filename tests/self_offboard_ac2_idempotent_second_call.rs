//! AC2 — Given an already-offboarded tenant, When `host.self_offboard` is
//! called again with the same key, Then it returns a clean idempotent
//! result, not a crash or ambiguous error. (Superseded in shape by
//! PRD-mcphost-self-offboard-success-envelope: the second call is now the
//! `already_offboarded` envelope.)
//!
//! `control::self_offboard`'s own doc comment explains the mechanism: a
//! disabled tenant's key never reaches `dispatch_tenant_tool` a second
//! time -- `resolve_auth` refuses it first, with the same well-typed
//! `tenant_disabled` error every other `host.*` call already returns for a
//! disabled tenant's key (see `ac08_admin_disable_and_forbidden.rs`). This
//! test pins that the second call is exactly that -- a typed, well-formed
//! error, not a 500/panic/ambiguous shape -- and that the server keeps
//! serving other tenants afterward (no corruption from the "double
//! offboard").

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn second_self_offboard_call_is_a_clean_typed_refusal_not_a_crash() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Double Leaver").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("first host.self_offboard must succeed");

    // Second call with the same (now-disabled) key: the idempotent
    // `already_offboarded` envelope (PRD-mcphost-self-offboard-success-
    // envelope R2), never a crash and never a second real offboard.
    let again = client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("a repeat self_offboard must be the idempotent envelope, not an error");
    let again = common::extract_structured(&again);
    assert_eq!(again["disabled"], json!(true));
    assert_eq!(again["already_offboarded"], json!(true));

    // The server is still healthy for everyone else -- an admin can still
    // list tenants, and a fresh signup still works, proving the double-call
    // didn't wedge or corrupt shared state.
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants must still work after a double self_offboard");
    let (other_ns, _other_key) = signup(&server.base_url, "Unrelated Tenant").await;
    assert_ne!(other_ns, ns);
}
