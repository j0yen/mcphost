//! AC2 — Given an already-offboarded tenant, When `host.self_offboard` is
//! called again with the same key, Then it returns a clean idempotent
//! result, not a crash or ambiguous error.
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

    // Second call with the same (now-disabled) key: a typed error, never a
    // crash, and never a fresh "success" body claiming to have offboarded
    // an already-gone tenant again.
    let err = client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect_err("a second self_offboard with a disabled key must be refused, not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tenant_disabled"));

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
