//! PRD-mcphost-first-call-gift
//! AC8 — Given a brand-new implicit tenant (no `signup` call), When its
//! first `host.*` call succeeds, Then the `onboarding` envelope carries
//! `memory_line` and `memory_hint` and no `welcome_back`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn implicit_tenants_first_call_carries_memory_line_and_no_welcome_back() {
    let server = TestServer::start().await;

    // Given: a fresh session, no `Authorization` header, no `tenant_key`
    // argument ever sent on it -- no `signup` call either.
    let session = McpClient::new(&server.base_url).with_session_continuity();

    // When: the very first call is a bare host.* call.
    let publish = session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("a bare host.tool_publish call must succeed");
    let published = extract_structured(&publish);

    let onboarding = &published["onboarding"];
    assert!(onboarding.get("tenant").is_some(), "{published}");
    let memory_line = onboarding["memory_line"].as_str().expect("onboarding.memory_line");
    assert!(memory_line.len() <= 200, "memory_line too long: {memory_line:?}");
    let url = onboarding["url"].as_str().expect("onboarding.url");
    assert!(
        memory_line.contains(url),
        "memory_line must name this tenant's own URL: {memory_line:?} vs {url:?}"
    );
    assert!(
        onboarding.get("memory_hint").is_some(),
        "onboarding must carry memory_hint: {published}"
    );

    assert!(
        published.get("welcome_back").is_none(),
        "a brand-new implicit tenant's first call must carry no welcome_back: {published}"
    );
}
