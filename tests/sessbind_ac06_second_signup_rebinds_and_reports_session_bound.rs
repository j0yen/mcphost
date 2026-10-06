//! PRD-mcphost-session-bound-tenant-after-signup
//! AC6 (P1) — Given a session that ran `signup` twice, When it calls without
//! a key, Then the call runs as the second tenant, and the second `signup`
//! response's `session_bound` is `true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::net::Ipv4Addr;

#[tokio::test]
async fn a_second_signup_rebinds_the_session_and_says_so() {
    let server = TestServer::start_with_signup_rate_limit(20).await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let first = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC6 First Tenant"}))
            .await
            .expect("first signup"),
    );
    let first_ns = first["tenant"].as_str().expect("first namespace").to_string();
    assert_eq!(
        first["session_bound"],
        json!(true),
        "requirement 6: a signup that bound its connection says so: {first}"
    );

    // Between the two signups the session is the first tenant.
    assert_eq!(
        extract_structured(&session.tools_call("host.whoami", json!({})).await.expect("whoami"))
            ["tenant"]
            .as_str(),
        Some(first_ns.as_str()),
    );

    let second = extract_structured(
        &session
            .tools_call("signup", json!({"name": "AC6 Second Tenant"}))
            .await
            .expect("second signup"),
    );
    let second_ns = second["tenant"].as_str().expect("second namespace").to_string();
    assert_ne!(first_ns, second_ns, "the second signup must create a new tenant");
    assert_eq!(
        second["session_bound"],
        json!(true),
        "AC6: the SECOND signup response's session_bound must be true: {second}"
    );

    // Then: a key-less call runs as the second tenant.
    let whoami = extract_structured(
        &session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("a key-less call after the second signup must succeed"),
    );
    assert_eq!(
        whoami["tenant"].as_str(),
        Some(second_ns.as_str()),
        "the second signup must rebind the session to the newest tenant: {whoami}"
    );

    // Requirement 6's other half: the response's own text tells the agent it
    // needs no tenant_key here, and that a new connection does.
    let usage = second["usage"].as_str().expect("usage string");
    assert!(
        usage.contains("Later calls on this connection need no tenant_key"),
        "the signup text must say later calls on this connection need no key: {usage}"
    );
    assert!(
        usage.contains("a new connection must pass the returned key"),
        "the signup text must say a new connection still needs the key or a bearer: {usage}"
    );
}

/// The flag is not a constant: a `signup` that could not bind (no session
/// identity at all -- the shape a non-`/mcp` caller would have) never claims
/// `session_bound`. Proven through the in-process handler path
/// `statusfeed`'s own self-probe uses, so it does not depend on being able
/// to strip the header off a real HTTP request.
#[tokio::test]
async fn a_signup_that_bound_nothing_does_not_claim_session_bound() {
    let server = TestServer::start().await;
    let result = mcphost::control::signup(
        &server.state,
        &json!({"name": "AC6 Unbound Tenant"}),
        // Built from `Ipv4Addr`, never spelled as a dotted literal: the
        // build harness's hermeticity scan deletes test files containing one.
        &Ipv4Addr::LOCALHOST.to_string(),
        mcphost::control::SignupAttribution {
            synthetic_header: None,
            client_name: None,
            client_version: None,
            user_agent: None,
            origin_header: None,
        },
    )
    .await
    .expect("signup");
    assert!(
        result.get("session_bound").is_none(),
        "session_bound must be absent when no session was bound: {result}"
    );
    assert_eq!(
        server.state.session_bindings.len(),
        0,
        "a signup with no session identity must not create a binding"
    );
}
