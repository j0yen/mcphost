//! PRD-mcphost-implicit-signup
//! AC2 (P0) — Given the same session, When a second `host.*` call runs,
//! Then its result carries no `onboarding` field.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn second_call_on_the_same_session_has_no_onboarding_field() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let first = session
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("first bare call implicitly signs up");
    let first = extract_structured(&first);
    assert!(
        !first["onboarding"].is_null(),
        "sanity: the first call must carry onboarding: {first}"
    );

    // Second call, same session, still no Authorization header and no
    // tenant_key argument -- it must now resolve through the session
    // binding the first call created, not through implicit signup again.
    let second = session
        .tools_call("host.tool_list", json!({}))
        .await
        .expect("second call on the bound session");
    let second = extract_structured(&second);
    assert!(
        second.get("onboarding").is_none() || second["onboarding"].is_null(),
        "a later call on the same session must not repeat onboarding: {second}"
    );

    let before = server.state.db.list_tenants().await.unwrap().len();
    session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("third call, same session");
    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after, before,
        "a bound session must never mint a second tenant"
    );
}
