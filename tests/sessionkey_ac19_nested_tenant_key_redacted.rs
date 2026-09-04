//! PRD-mcphost-session-key
//! AC19 — Given a call carrying `tenant_key` nested inside an object within
//! `args`, When the arguments are recorded or echoed by `host.tool_test`,
//! Then the nested value is redacted, proving redaction is by key name
//! rather than by matching the top-level value.

mod common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_tenant_key_nested_inside_the_tools_own_args_is_redacted_too() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Nested Key Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
            }),
        )
        .await
        .expect("publish");

    // `tenant_key` here has nothing to do with authentication -- it is a
    // field nested two levels inside the tool's own `args`, coincidentally
    // sharing the auth argument's name. A value-based redaction (matching
    // known secret values) would never catch this; only a by-key-name pass
    // does.
    let result = client
        .tools_call(
            "host.tool_test",
            json!({
                "name": "hello",
                "args": {"outer": {"tenant_key": "coincidental-nested-value"}},
            }),
        )
        .await
        .expect("host.tool_test");
    let structured = extract_structured(&result);
    let dump = structured.to_string();

    assert!(
        !dump.contains("coincidental-nested-value"),
        "the nested tenant_key value must be redacted from host.tool_test's echo: {dump}"
    );
    assert!(
        dump.contains("***"),
        "expected the redaction marker in place of the nested value: {dump}"
    );
}
