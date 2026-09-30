//! PRD-mcphost-session-bound-tenant-after-signup
//! AC2 (P0) — Given the session in AC1, When it calls each of
//! `host.tool_publish`, `host.catalog.get`, `host.state.table_create`,
//! `host.state.insert` and `host.agent.profile_set` with a body valid for
//! each and no `tenant_key`, Then every call runs as the bound tenant (one
//! parameterized test, 5 cases -- plus the `host.tool_share` that makes
//! `host.catalog.get`'s subject a public tool, key-less on the same session).
//!
//! "Runs as the bound tenant" is checked two ways per case: the call itself
//! must succeed key-less, and its effect must land on the tenant this
//! session created -- read back afterwards with that tenant's own key from a
//! completely separate, session-less connection, so a case could not pass by
//! writing into some other tenant's namespace.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};

#[tokio::test]
async fn five_tools_run_as_the_bound_tenant_with_no_tenant_key() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let signed_up = session
        .tools_call("signup", json!({"name": "AC2 Tenant"}))
        .await
        .expect("signup");
    let signed_up = extract_structured(&signed_up);
    let namespace = signed_up["tenant"].as_str().expect("tenant namespace").to_string();
    let key = signed_up["key"].as_str().expect("tenant key").to_string();

    let cases: Vec<(&str, Value)> = vec![
        (
            "host.tool_publish",
            json!({
                "name": "ac2tool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}},
            }),
        ),
        // `host.tool_share` makes the tool public, which is the only state
        // in which `host.catalog.get` resolves one (`sharing::catalog_get`
        // reads `get_public_tool`) -- a body valid for the tool, per AC2.
        ("host.tool_share", json!({"name": "ac2tool", "visibility": "public"})),
        ("host.catalog.get", json!({"full_name": format!("{namespace}.ac2tool")})),
        (
            "host.state.table_create",
            json!({"name": "ac2_table", "schema": {"id": "text", "v": "integer"}, "primary_key": "id"}),
        ),
        (
            "host.state.insert",
            json!({"table": "ac2_table", "rows": [{"id": "r1", "v": 1}]}),
        ),
        ("host.agent.profile_set", json!({"handle": "ac2agent"})),
    ];

    for (name, arguments) in &cases {
        let result = session
            .tools_call(name, arguments.clone())
            .await
            .unwrap_or_else(|e| {
                panic!("{name} must run key-less as the bound tenant, not be refused: {e:?}")
            });
        assert!(
            !format!("{result}").contains("tenant_key_missing"),
            "{name}'s result must not carry a tenant_key refusal: {result}"
        );
    }

    // Every effect above must belong to the tenant this session created.
    // Read back over a fresh, session-less connection authenticated with
    // that tenant's own key -- so nothing here can be satisfied by the same
    // session simply talking to itself.
    let keyed = McpClient::with_bearer(&server.base_url, &key);

    let catalog = extract_structured(
        &keyed
            .tools_call("host.catalog.get", json!({"full_name": format!("{namespace}.ac2tool")}))
            .await
            .expect("the published tool must belong to the bound tenant"),
    );
    assert_eq!(catalog["kind"].as_str(), Some("echo"), "{catalog}");

    let rows = extract_structured(
        &keyed
            .tools_call("host.state.query", json!({"table": "ac2_table"}))
            .await
            .expect("the created table and inserted row must belong to the bound tenant"),
    );
    assert_eq!(
        rows["rows"].as_array().map(Vec::len),
        Some(1),
        "the key-less host.state.insert must have written into the bound tenant's table: {rows}"
    );

    let whoami = extract_structured(
        &keyed.tools_call("host.whoami", json!({})).await.expect("host.whoami"),
    );
    assert_eq!(whoami["tenant"].as_str(), Some(namespace.as_str()), "{whoami}");

    // And the same five bodies on a connection that never signed up are
    // still refused, so none of the successes above came from the tools
    // simply not requiring a tenant any more.
    for (name, arguments) in &cases {
        let anonymous = McpClient::new(&server.base_url);
        let err = anonymous
            .tools_call(name, arguments.clone())
            .await
            .err()
            .unwrap_or_else(|| {
                panic!("{name} must still require a tenant on an unbound connection")
            });
        assert_eq!(
            err.error_code.as_deref(),
            Some("tenant_key_missing"),
            "{name} on an unbound connection must be refused exactly as at v0.60.35: {err:?}"
        );
    }
}
