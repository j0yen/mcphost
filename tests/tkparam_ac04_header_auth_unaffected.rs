//! PRD-mcphost-tenant-key-missing-is-invalid-params
//! AC4 (P0) — Given a call to `host.tool_publish` with a valid
//! `Authorization: Bearer` header and no `tenant_key` argument, When the
//! server responds, Then the call succeeds (or fails only on the spec
//! body) and no response anywhere carries `error_code` `tenant_key_missing`;
//! and the same holds for the other five tools with a body that is valid
//! for each.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn header_auth_never_sees_tenant_key_missing() {
    let server = TestServer::start_with_signup_rate_limit(20).await;

    let cases: &[(&str, Value)] = &[
        (
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        ),
        ("host.catalog.search", json!({"q": "anything"})),
        ("host.catalog.get", json!({"full_name": "nobody.nothing"})),
        (
            "host.state.table_create",
            json!({"name": "t1", "schema": {"col": "text"}}),
        ),
        (
            "host.state.insert",
            json!({"table": "t1", "rows": [{"col": "x"}]}),
        ),
        ("host.agent.profile_set", json!({})),
    ];

    for (tool, args) in cases {
        let (_ns, key) = signup(&server.base_url, &format!("Bearer Tenant for {tool}")).await;
        let client = McpClient::with_bearer(&server.base_url, &key);

        match client.tools_call(tool, args.clone()).await {
            Ok(_) => {}
            Err(err) => assert_ne!(
                err.error_code.as_deref(),
                Some("tenant_key_missing"),
                "{tool} with a valid Bearer header and no tenant_key must never refuse as \
                 tenant_key_missing: {err:?}"
            ),
        }
    }
}
