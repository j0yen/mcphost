//! PRD-mcphost-session-bound-tenant-after-signup
//! AC5 (P0) — Given a bound session, When a valid `Authorization: Bearer`
//! for tenant T2 is presented on a call, Then the call runs as T2 (header
//! wins) and the binding is unchanged for later header-less calls.
//!
//! The bearer call and the trailing header-less call both travel on the SAME
//! session id as the `signup` that created the binding (see
//! `McpClient::joining_session_of`), which is what makes the second half --
//! "the binding is unchanged" -- meaningful: a test that used an unrelated
//! connection for either one would pass whether or not the bearer path
//! clobbered the binding.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn a_bearer_wins_on_the_bound_session_and_leaves_the_binding_intact() {
    let server = TestServer::start_with_signup_rate_limit(20).await;

    let session = McpClient::new(&server.base_url).with_session_continuity();
    let t1 = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC5 Bound Tenant"}))
            .await
            .expect("signup T1"),
    );
    let t1_ns = t1["tenant"].as_str().expect("T1 namespace").to_string();

    let (t2_ns, t2_key) = common::signup(&server.base_url, "AC5 Bearer Tenant").await;
    assert_ne!(t1_ns, t2_ns);

    // Given: the session is bound to T1.
    assert_eq!(
        extract_structured(&session.tools_call("host.whoami", json!({})).await.expect("whoami"))
            ["tenant"]
            .as_str(),
        Some(t1_ns.as_str()),
    );

    // When: a valid bearer for T2 on that same session.
    let bearer_on_same_session =
        McpClient::with_bearer(&server.base_url, &t2_key).joining_session_of(&session);
    assert_eq!(
        bearer_on_same_session.session_id(),
        session.session_id(),
        "the bearer call must genuinely travel on the bound session, not a new one"
    );
    let via_bearer = extract_structured(
        &bearer_on_same_session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the bearer call must succeed"),
    );
    assert_eq!(
        via_bearer["tenant"].as_str(),
        Some(t2_ns.as_str()),
        "the Authorization header must win over the session's binding: {via_bearer}"
    );

    // Then: "the binding is unchanged for later header-less calls" -- on the
    // very same session the bearer just used.
    let after = extract_structured(
        &session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the header-less call after the bearer must still succeed"),
    );
    assert_eq!(
        after["tenant"].as_str(),
        Some(t1_ns.as_str()),
        "a bearer passing through must not rebind the session to T2: {after}"
    );
}

/// The precedence rule's first step in its other direction: a bearer that
/// does NOT resolve still wins over the binding -- an unusable header is not
/// silently downgraded to the session's tenant. (`resolve_auth` returns
/// `Auth::Invalid` for it, and `call_tool` only consults the binding when the
/// header path came back `Anonymous`.)
#[tokio::test]
async fn an_unusable_bearer_is_not_downgraded_to_the_binding() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "AC5 Invalid Bearer Tenant"}))
        .await
        .expect("signup");

    let bogus_bearer = McpClient::with_bearer(&server.base_url, "not-a-real-key-at-all")
        .joining_session_of(&session);
    let err = bogus_bearer
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("an unusable Authorization header must be refused, not fall back to the binding");
    assert_eq!(
        err.error_code.as_deref(),
        Some("bearer_invalid"),
        "the header path's own refusal must be unchanged: {err:?}"
    );

    // The binding itself survives the refused call.
    let after = extract_structured(
        &session.tools_call("host.whoami", json!({})).await.expect("key-less whoami"),
    );
    assert!(after["tenant"].as_str().is_some(), "{after}");
}
