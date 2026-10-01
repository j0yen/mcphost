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
async fn bare_host_tool_publish_implicitly_signs_up_and_returns_onboarding() {
    let server = TestServer::start().await;
    let before = server.state.db.list_tenants().await.unwrap().len();

    // A spec-compliant streamable-HTTP client with no Authorization header
    // and no tenant_key argument at all -- exactly the bare
    // `claude mcp add --transport http mcphost https://mcphost.dev/mcp`
    // connection the PRD's grounding describes, with session continuity so
    // the binding this call creates is visible to the AC's own
    // `host.whoami` check.
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let published = session
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("a bare host.tool_publish must now succeed instead of tenant_key_missing");
    let published = extract_structured(&published);

    let onboarding = &published["onboarding"];
    let tenant_ns = onboarding["tenant"]
        .as_str()
        .unwrap_or_else(|| panic!("onboarding.tenant must be present: {published}"))
        .to_string();
    let url = onboarding["url"]
        .as_str()
        .unwrap_or_else(|| panic!("onboarding.url must be present: {published}"));

    // The real call's own result is still there (requirement 2: the
    // caller sees its real result, not a separate "signed up" response) --
    // qualified under the tenant it was just published to, same shape a
    // call already carrying a `tenant_key` gets from `host.tool_publish`.
    assert_eq!(
        published["name"],
        json!(format!("{tenant_ns}.hello")),
        "the echo tool must actually be published: {published}"
    );
    assert!(
        url.starts_with(&format!("{}/u/", server.base_url)) && url.ends_with("/mcp"),
        "onboarding.url must be a /u/<secret>/mcp link: {url}"
    );

    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(
        after,
        before + 1,
        "exactly one tenant must be created implicitly"
    );

    let whoami = session
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the now-bound session");
    let whoami = extract_structured(&whoami);
    assert_eq!(whoami["tenant"], json!(tenant_ns), "{whoami}");
    assert_eq!(
        whoami["source"],
        json!("implicit"),
        "the implicitly-created tenant must be stamped source: implicit: {whoami}"
    );
}
