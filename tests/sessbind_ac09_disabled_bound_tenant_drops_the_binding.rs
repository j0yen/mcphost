//! PRD-mcphost-session-bound-tenant-after-signup
//! AC9 (P0, as landed) — Given a bound session whose tenant the operator
//! disables, When it calls without a key, Then the response is
//! `TenantDisabled`'s existing error and a following call on the same
//! session returns `tenant_key_missing` (binding dropped).
//!
//! PRD-mcphost-implicit-signup: the dropped-binding half no longer reads
//! `tenant_key_missing` -- that next key-less call on the (now unbound
//! again) session implicitly signs up a brand-new tenant instead
//! (requirement 4). The real invariant -- a dropped binding never
//! resurrects the disabled tenant -- still holds: the session now simply
//! runs as a DIFFERENT, fresh implicit tenant from that point on.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn disabling_the_bound_tenant_surfaces_tenant_disabled_then_drops_the_binding() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let signed_up = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC9 Tenant"}))
            .await
            .expect("signup"),
    );
    let namespace = signed_up["tenant"].as_str().expect("namespace").to_string();

    // Given: the session really is bound -- a key-less call runs as it.
    assert_eq!(
        extract_structured(&session.tools_call("host.whoami", json!({})).await.expect("whoami"))
            ["tenant"]
            .as_str(),
        Some(namespace.as_str()),
    );
    assert_eq!(server.state.session_bindings.len(), 1);

    // The operator disables it.
    McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("admin.tenant_disable", json!({"tenant": namespace}))
        .await
        .expect("admin.tenant_disable");

    // When: the same session calls without a key.
    let err = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a bound session whose tenant is disabled must be refused");
    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_disabled"),
        "AC9: the existing TenantDisabled error, not a generic refusal: {err:?}"
    );

    // Then: the binding is dropped, so the NEXT call on the same session is
    // an ordinary anonymous refusal.
    assert_eq!(
        server.state.session_bindings.len(),
        0,
        "the binding must be dropped, not left pointing at a disabled tenant"
    );
    let next = extract_structured(
        &session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the next call on the same session now implicitly signs up a fresh tenant"),
    );
    let fresh_namespace = next["tenant"].as_str().expect("fresh namespace").to_string();
    assert_ne!(
        fresh_namespace, namespace,
        "AC9's second half: the dropped binding must never resurrect the disabled tenant: {next}"
    );

    // Re-enabling the ORIGINAL (now-abandoned) tenant does not resurrect
    // it on this session either -- a binding is only ever created by a
    // signup/redeem/implicit-signup on the session itself (requirement 5).
    //
    // PRD-mcphost-session-bound-tenant-key requirement 2 supersedes this
    // block's original "the session stays bound" expectation: the call at
    // line 64 above was this session's own (fresh) implicit signup, so
    // THIS call is a SECOND key-less call on it -- by default refused and
    // named, naming the fresh tenant, never the re-enabled original one.
    McpClient::with_bearer(&server.base_url, ADMIN_KEY)
        .tools_call("admin.tenant_enable", json!({"tenant": namespace}))
        .await
        .expect("admin.tenant_enable");
    let after_enable = session.tools_call("host.whoami", json!({})).await;
    let err = after_enable.expect_err("a second key-less call on the fresh implicit tenant's session must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(
        err.data.get("tenant").and_then(serde_json::Value::as_str),
        Some(fresh_namespace.as_str()),
        "re-enabling the original tenant must not resurrect it on this session: {err:?}"
    );
}
