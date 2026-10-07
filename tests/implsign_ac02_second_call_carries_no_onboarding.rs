//! PRD-mcphost-implicit-signup
//! AC2 (P0) — Given the same session [that just implicitly signed up], When
//! a second `host.*` call runs, Then its result carries no `onboarding`
//! field.
//!
//! PRD-mcphost-session-bound-tenant-key requirement 2 (AC2) supersedes this
//! AC's original "and the call succeeds" half: a second key-less call on
//! the very same session no longer silently rides the tenant the first one
//! implicitly created -- by default it's refused and named instead, so
//! that it can carry no `onboarding` (or any other field) at all. The
//! dedicated proof for the new refusal shape (`data.tenant`, the hint,
//! `signup_events`) is `sessbind_ac15_second_bare_call_is_refused_and_named.rs`;
//! this file keeps its own original name/AC pointer and narrows to what
//! still holds of its own Then -- no second tenant, no onboarding leaking
//! through on a refusal.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn second_host_call_on_the_same_session_is_refused_not_silently_served() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    // The first call -- bare, anonymous -- implicitly signs this session up
    // and carries onboarding.
    let first = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("the first bare host.* call must succeed");
    let first = extract_structured(&first);
    assert!(
        first.get("onboarding").is_some(),
        "the first call that creates the tenant must carry onboarding: {first}"
    );
    let tenant = first["tenant"].as_str().expect("tenant").to_string();

    // A second host.* call on the very same (now memoed) connection: no
    // onboarding leaks through, because the call itself is refused.
    let second = session.tools_call("host.usage", json!({})).await;
    let err = second.expect_err("a second key-less call on an already-memoed session must be refused");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"));
    assert_eq!(
        err.data.get("tenant").and_then(serde_json::Value::as_str),
        Some(tenant.as_str()),
        "the refusal must name the tenant this session already is: {err:?}"
    );

    // Sanity: still only one tenant, ever.
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "no second tenant must have been created");
    assert_eq!(tenants[0].namespace, tenant);
}
