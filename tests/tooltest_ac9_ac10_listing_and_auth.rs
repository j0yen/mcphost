//! PRD-mcphost-tool-test:
//! AC9 (P1) — Given an unauthenticated connection, When it lists tools,
//! Then `host.spec_test` is absent, and calling it is gated like any
//! other `host.*` tool.
//! AC10 (P1) — Given an authenticated tenant, When it lists tools, Then
//! `host.spec_test` appears with an input schema documenting `kind`,
//! `spec`, and `invocations`.
//!
//! PRD-mcphost-implicit-signup: "gated like any other host.* tool" used to
//! mean "refused tenant_key_missing" -- now it means "implicitly signs up
//! and runs as that new tenant", same as every other bare host.*/billing.*
//! call on /mcp (see tests/implsign_ac01_*.rs). This AC's real point --
//! host.spec_test follows the exact same auth gate as the rest of the
//! host.* surface, not a gate of its own -- still holds; only which
//! outcome that shared gate now produces changed.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn absent_from_the_anonymous_list_and_refused_when_called() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let tools = client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"host.spec_test"),
        "host.spec_test must not be listed for an anonymous caller: {names:?}"
    );

    // PRD-mcphost-implicit-signup: a bare call now implicitly signs up and
    // runs as the new tenant -- the empty `spec: {}` is itself invalid for
    // an echo tool, so this still ends in a refusal, just never
    // tenant_key_missing (the whole point this AC's gate no longer reaches).
    let err = client
        .tools_call(
            "host.spec_test",
            json!({"kind": "echo", "spec": {}, "invocations": []}),
        )
        .await
        .expect_err("an empty echo spec must still be rejected, just not for auth reasons");
    assert_ne!(
        err.error_code.as_deref(),
        Some("tenant_key_missing"),
        "host.spec_test must be gated exactly like any other host.* tool, \
         which no longer means tenant_key_missing for a bare call: {err:?}"
    );
}

#[tokio::test]
async fn present_for_an_authenticated_tenant_with_the_documented_schema() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Listing Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tools = client.tools_list().await.expect("tools/list");
    let spec_test = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!("host.spec_test"))
        .expect("host.spec_test must be listed for an authenticated tenant")
        .clone();
    let props = spec_test["inputSchema"]["properties"]
        .as_object()
        .expect("properties object");
    assert!(props.contains_key("kind"));
    assert!(props.contains_key("spec"));
    assert!(props.contains_key("invocations"));
}
