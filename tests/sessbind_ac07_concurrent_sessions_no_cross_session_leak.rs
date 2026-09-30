//! PRD-mcphost-session-bound-tenant-after-signup
//! AC7 (P0) — Given two concurrent sessions, When session A signs up and
//! session B (never signed up) calls without a key at the same time, Then A's
//! calls succeed and B is refused with `tenant_key_missing`; no binding leaks
//! across sessions.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn a_signs_up_and_succeeds_while_b_is_refused_at_the_same_time() {
    let server = TestServer::start_with_signup_rate_limit(20).await;

    let a = McpClient::new(&server.base_url).with_session_continuity();
    let b = McpClient::new(&server.base_url).with_session_continuity();

    // B opens its session first (and never signs up on it).
    b.initialize().await;

    let a_ns = extract_structured(
        &a.tools_call("signup", json!({"name": "AC7 Session A"}))
            .await
            .expect("A signs up"),
    )["tenant"]
        .as_str()
        .expect("A's namespace")
        .to_string();

    // Concurrently: A's key-less call and B's key-less call.
    let (a_result, b_result) = tokio::join!(
        a.tools_call("host.whoami", json!({})),
        b.tools_call("host.whoami", json!({})),
    );

    let a_result = a_result.expect("A's key-less call must succeed as its own tenant");
    assert_eq!(
        extract_structured(&a_result)["tenant"].as_str(),
        Some(a_ns.as_str()),
        "A must run as the tenant its own session created: {a_result}"
    );

    let b_err = b_result.expect_err("B never signed up, so its key-less call must be refused");
    assert_eq!(
        b_err.error_code.as_deref(),
        Some("tenant_key_missing"),
        "B must get the unchanged anonymous refusal, never A's tenant: {b_err:?}"
    );

    assert_ne!(
        a.session_id(),
        b.session_id(),
        "two concurrent connections must hold two distinct server-issued session ids"
    );
    assert_eq!(
        server.state.session_bindings.len(),
        1,
        "only the session that actually signed up may hold a binding"
    );

    // And B still cannot reach A's tenant after the fact, even having seen
    // A's namespace go by.
    let b_again = b
        .tools_call("host.catalog.search", json!({}))
        .await
        .expect_err("B stays anonymous for every later call too");
    assert_eq!(b_again.error_code.as_deref(), Some("tenant_key_missing"), "{b_again:?}");
}

/// The same isolation under real contention: eight sessions sign up at once
/// and each must resolve key-less to its own tenant, never a neighbour's.
#[tokio::test]
async fn eight_concurrent_signups_each_bind_only_their_own_session() {
    let server = TestServer::start_with_signup_rate_limit(100).await;

    let sessions: Vec<std::sync::Arc<McpClient>> = (0..8)
        .map(|_| std::sync::Arc::new(McpClient::new(&server.base_url).with_session_continuity()))
        .collect();

    let mut signups = tokio::task::JoinSet::new();
    for (i, client) in sessions.iter().enumerate() {
        let client = std::sync::Arc::clone(client);
        signups.spawn(async move {
            let result = client
                .tools_call("signup", json!({"name": format!("AC7 Concurrent {i}")}))
                .await
                .expect("concurrent signup");
            let ns = extract_structured(&result)["tenant"]
                .as_str()
                .expect("namespace")
                .to_string();
            (i, ns)
        });
    }
    let mut namespaces = vec![String::new(); sessions.len()];
    while let Some(joined) = signups.join_next().await {
        let (i, ns) = joined.expect("signup task");
        namespaces[i] = ns;
    }

    let mut checks = tokio::task::JoinSet::new();
    for (i, client) in sessions.iter().enumerate() {
        let client = std::sync::Arc::clone(client);
        checks.spawn(async move {
            let result = client
                .tools_call("host.whoami", json!({}))
                .await
                .expect("key-less whoami on a bound session");
            let ns = extract_structured(&result)["tenant"]
                .as_str()
                .expect("namespace")
                .to_string();
            (i, ns)
        });
    }
    let mut resolved = vec![String::new(); sessions.len()];
    while let Some(joined) = checks.join_next().await {
        let (i, ns) = joined.expect("whoami task");
        resolved[i] = ns;
    }

    assert_eq!(resolved, namespaces, "no session may resolve to another session's tenant");
    assert_eq!(server.state.session_bindings.len(), 8);
}
