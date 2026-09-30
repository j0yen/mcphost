//! PRD-mcphost-session-bound-tenant-after-signup
//! AC4 (P0) — Given a bound session, When it calls with an explicit valid
//! `tenant_key` of a different tenant, Then the call runs as the explicit
//! key's tenant; and When it calls with an invalid `tenant_key`, Then the
//! response is `tenant_key_invalid`, not the bound tenant.
//!
//! Both halves run on a session that genuinely holds a binding (it signed up
//! as T1 and its key-less calls resolve to T1), so each one is a real
//! override of a live binding rather than a check on a connection where no
//! binding could have interfered in the first place.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn an_explicit_key_overrides_the_binding_and_an_invalid_one_is_refused() {
    // Two signups from the same loopback source: lift the default 5/hour cap
    // out of the way (`state::SIGNUP_RATE_LIMIT_PER_HOUR`) the same way
    // `oauthsig_ac02` already does.
    let server = TestServer::start_with_signup_rate_limit(20).await;

    let session = McpClient::new(&server.base_url).with_session_continuity();
    let t1 = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC4 Bound Tenant"}))
            .await
            .expect("signup T1"),
    );
    let t1_ns = t1["tenant"].as_str().expect("T1 namespace").to_string();

    // A second tenant, created on a different connection entirely, so its
    // key is genuinely "of a different tenant".
    let (t2_ns, t2_key) = common::signup(&server.base_url, "AC4 Other Tenant").await;
    assert_ne!(t1_ns, t2_ns);

    // Given: the session really is bound -- a key-less call runs as T1.
    let bound = extract_structured(
        &session.tools_call("host.whoami", json!({})).await.expect("key-less whoami"),
    );
    assert_eq!(bound["tenant"].as_str(), Some(t1_ns.as_str()), "{bound}");

    // When: an explicit valid tenant_key of a DIFFERENT tenant, on that same
    // bound session. Then: the call runs as the explicit key's tenant.
    let overridden = extract_structured(
        &session
            .tools_call("host.whoami", json!({"tenant_key": t2_key}))
            .await
            .expect("an explicit valid key must be accepted on a bound session"),
    );
    assert_eq!(
        overridden["tenant"].as_str(),
        Some(t2_ns.as_str()),
        "the explicit tenant_key must win over the session's binding: {overridden}"
    );

    // When: an invalid tenant_key. Then: `tenant_key_invalid` -- the binding
    // must never silently rescue a key the caller actually sent.
    let err = session
        .tools_call("host.whoami", json!({"tenant_key": "not-a-real-key-at-all"}))
        .await
        .expect_err("an invalid explicit key must be refused, not replaced by the binding");
    assert_eq!(
        err.error_code.as_deref(),
        Some("tenant_key_invalid"),
        "an invalid tenant_key on a bound session must still read tenant_key_invalid: {err:?}"
    );

    // And the binding itself is untouched by either override: the next
    // key-less call on the same session is still T1.
    let still_bound = extract_structured(
        &session.tools_call("host.whoami", json!({})).await.expect("key-less whoami again"),
    );
    assert_eq!(
        still_bound["tenant"].as_str(),
        Some(t1_ns.as_str()),
        "neither an explicit key nor a rejected one may change the binding: {still_bound}"
    );
}
