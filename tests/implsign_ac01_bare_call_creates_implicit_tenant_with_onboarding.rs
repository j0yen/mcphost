//! PRD-mcphost-implicit-signup
//! AC1 (P0) — Given a fresh streamable-HTTP session on `/mcp` with no
//! header, When `host.tool_publish` is called with a valid echo spec, Then
//! the tool is published under a new tenant, the result carries
//! `onboarding.url` and `onboarding.tenant`, and `host.whoami` on the same
//! session returns that tenant with `source: "implicit"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn bare_publish_call_mints_an_implicit_tenant_and_carries_onboarding() {
    let server = TestServer::start().await;

    // Given: a fresh session, no `Authorization` header, no `tenant_key`
    // argument ever sent on it.
    let session = McpClient::new(&server.base_url).with_session_continuity();

    // When: the very first call is `host.tool_publish` with a valid echo
    // spec -- no prior `signup`.
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
        .expect("a bare host.tool_publish call must succeed, not fail tenant_key_missing");
    let published = extract_structured(&publish);

    // Then: the tool is published under a new tenant...
    let qualified = published["name"].as_str().expect("qualified tool name");
    assert!(
        qualified.ends_with(".hello") && qualified != "hello",
        "the tool must be published under a real tenant namespace, got {qualified}"
    );
    let implicit_tenant = qualified.trim_end_matches(".hello").to_string();

    // ...and the result carries onboarding.url and onboarding.tenant.
    let onboarding = &published["onboarding"];
    assert_eq!(
        onboarding["tenant"].as_str(),
        Some(implicit_tenant.as_str()),
        "onboarding.tenant must name the tenant that was just created: {published}"
    );
    let url = onboarding["url"].as_str().expect("onboarding.url must be present");
    assert!(
        url.contains("/u/") && url.ends_with("/mcp"),
        "onboarding.url must be this tenant's own /u/{{secret}}/mcp URL, got {url}"
    );

    // Exactly one tenant exists, and it is the implicit one.
    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(tenants.len(), 1, "exactly one tenant must have been created");
    assert_eq!(tenants[0].namespace, implicit_tenant);

    // host.whoami on the SAME session returns that tenant with
    // source: "implicit".
    let whoami = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("a key-less host.whoami on the bound session must succeed");
    let whoami = extract_structured(&whoami);
    assert_eq!(
        whoami["tenant"].as_str(),
        Some(implicit_tenant.as_str()),
        "host.whoami must report the same tenant this session was bound to: {whoami}"
    );
    assert_eq!(
        whoami["source"].as_str(),
        Some("implicit"),
        "host.whoami must report source: implicit for an implicitly-created tenant: {whoami}"
    );
}
