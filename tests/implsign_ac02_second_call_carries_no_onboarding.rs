//! PRD-mcphost-implicit-signup
//! AC2 (P0) — Given the same session [that just implicitly signed up], When
//! a second `host.*` call runs, Then its result carries no `onboarding`
//! field.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn second_host_call_on_the_same_session_carries_no_onboarding() {
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

    // A second host.* call on the very same (now session-bound) connection.
    let second = session
        .tools_call("host.usage", json!({}))
        .await
        .expect("a session-bound call must succeed");
    let second = extract_structured(&second);
    assert!(
        second.get("onboarding").is_none(),
        "a second call on an already-bound session must carry no onboarding field: {second}"
    );

    // Sanity: still the same tenant, and still only one of it.
    let whoami_again = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("whoami again");
    assert_eq!(
        extract_structured(&whoami_again)["tenant"].as_str(),
        Some(tenant.as_str())
    );
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "no second tenant must have been created");
}
