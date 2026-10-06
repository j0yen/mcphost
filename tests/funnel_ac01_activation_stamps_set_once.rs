//! PRD-mcphost-activation-funnel
//! AC1 (P0) — Given a fresh external tenant, When it makes its first
//! authenticated call, publishes a tool, calls that tool, and reconnects
//! 15 minutes later on a new session, Then `first_call_unix`,
//! `first_publish_unix`, `first_own_call_unix`, and `second_session_unix`
//! are set once each and repeating the events leaves them unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn activation_stamps_are_set_once_and_survive_repeats() {
    let server = TestServer::start().await;

    // One session-continuity client, so `signup` and every call after it
    // share the session mcphost's own binding addresses -- the same
    // "signed up and kept talking" shape `sessbind_ac01` exercises.
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let signed_up = extract_structured(
        &session.tools_call("signup", json!({"name": "Funnel AC1 Tenant"})).await.expect("signup"),
    );
    let namespace = signed_up["tenant"].as_str().expect("namespace").to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert!(
        tenant.first_call_unix.is_none(),
        "signup itself resolves no auth, so it must not stamp first_call_unix yet: {tenant:?}"
    );
    let created_unix = tenant.created_unix.expect("created_unix set at signup");

    // First authenticated call of any kind: a key-less publish on the
    // bound session.
    session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "funnel_ac01_tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}},
            }),
        )
        .await
        .expect("publish");

    let qualified = format!("{namespace}.funnel_ac01_tool");
    session.tools_call(&qualified, json!({"msg": "hi"})).await.expect("own call");

    let after_first_round = server
        .state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    let first_call_unix = after_first_round.first_call_unix.expect("first_call_unix set");
    let first_publish_unix = after_first_round.first_publish_unix.expect("first_publish_unix set");
    let first_own_call_unix = after_first_round.first_own_call_unix.expect("first_own_call_unix set");
    assert!(
        after_first_round.second_session_unix.is_none(),
        "no reconnect yet: {after_first_round:?}"
    );

    // Repeating every event must leave the stamps unchanged.
    session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "funnel_ac01_tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}},
            }),
        )
        .await
        .expect("republish");
    session.tools_call(&qualified, json!({"msg": "again"})).await.expect("own call again");

    let after_repeat = server
        .state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(after_repeat.first_call_unix, Some(first_call_unix), "{after_repeat:?}");
    assert_eq!(after_repeat.first_publish_unix, Some(first_publish_unix), "{after_repeat:?}");
    assert_eq!(after_repeat.first_own_call_unix, Some(first_own_call_unix), "{after_repeat:?}");

    // "Reconnects 15 minutes later on a new session": backdate
    // `created_unix` by 15 minutes (the test harness's own stand-in for a
    // real sleep -- same convention as `set_oauth_grant_created_unix_for_test`),
    // mint this tenant's personal URL on the ORIGINAL (bound) session, then
    // hit that URL from a brand-new, session-continuity-less client.
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "created_unix", created_unix - 900)
        .await
        .expect("backdate created_unix");

    let rotated = extract_structured(
        &session.tools_call("host.key_rotate", json!({})).await.expect("key_rotate"),
    );
    let url = rotated["url"].as_str().expect("key_rotate returns a url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let reconnected = McpClient::new(&server.base_url).with_path(&path);
    reconnected.tools_call("host.whoami", json!({})).await.expect("reconnect over the URL secret");

    let after_reconnect = server
        .state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    let second_session_unix = after_reconnect.second_session_unix.expect("second_session_unix set");

    // Repeating the reconnect (a THIRD, independent session) leaves it
    // unchanged.
    let reconnected_again = McpClient::new(&server.base_url).with_path(&path);
    reconnected_again.tools_call("host.whoami", json!({})).await.expect("reconnect again");

    let after_reconnect_again = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(
        after_reconnect_again.second_session_unix,
        Some(second_session_unix),
        "{after_reconnect_again:?}"
    );
    // Every other stamp is still exactly what the first round set.
    assert_eq!(after_reconnect_again.first_call_unix, Some(first_call_unix));
    assert_eq!(after_reconnect_again.first_publish_unix, Some(first_publish_unix));
    assert_eq!(after_reconnect_again.first_own_call_unix, Some(first_own_call_unix));
}
