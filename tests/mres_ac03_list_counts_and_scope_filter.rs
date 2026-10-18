//! AC3 (P0) -- PRD-mcphost-resources-for-tenant-data. Given a tenant with 2
//! tables, 1 doc, 1 run, 2 tools, When `resources/list` is sent with the key
//! in the Authorization header, Then 6 entries are returned; Given an OAuth
//! session scoped `tables:read`, Then exactly the 2 table entries.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

/// Consent + token exchange for `scope` on the tenant's per-tenant resource.
async fn token_for_scope(server: &TestServer, ns: &str, key: &str, scope: &str) -> String {
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, ns);
    let (_client_id, token) = common::oauth_consent_and_token_scoped(&server.base_url, &resource, scope, &[("tenant_key", key)]).await;
    token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}")).to_string()
}

fn uris(reply: &Value) -> Vec<String> {
    reply["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("no resources array: {reply}"))
        .iter()
        .map(|r| r["uri"].as_str().expect("uri").to_string())
        .collect()
}

#[tokio::test]
async fn mres_ac03_header_key_lists_six_and_a_tables_scope_lists_two() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "mres ac03").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for table in ["orders", "customers"] {
        client
            .tools_call("host.table.create", json!({"name": table, "columns": {"id": "integer"}}))
            .await
            .expect("create table");
    }
    client
        .tools_call("host.docs.put", json!({"name": "notes.md", "content": "# notes"}))
        .await
        .expect("put doc");
    for tool in ["alpha", "beta"] {
        client
            .tools_call("host.tool_publish", json!({"name": tool, "kind": "echo", "spec": {"schema": {"type": "object"}}}))
            .await
            .expect("publish");
    }
    client.tools_call(&format!("{ns}.alpha"), json!({"x": 1})).await.expect("call makes one run");
    client
        .tools_call("host.oauth.scope_set", json!({"name": "tables:read", "description": "Read tables"}))
        .await
        .expect("scope_set");

    let listed = client.rpc("resources/list", json!({})).await;
    let all = uris(&listed);
    assert_eq!(all.len(), 6, "2 tables + 1 doc + 1 run + 2 tools: {all:?}");
    assert_eq!(all.iter().filter(|u| u.starts_with("mcphost://table/")).count(), 2, "{all:?}");
    assert_eq!(all.iter().filter(|u| u.starts_with("mcphost://doc/")).count(), 1, "{all:?}");
    assert_eq!(all.iter().filter(|u| u.starts_with("mcphost://run/") && u.ends_with("/log")).count(), 1, "{all:?}");
    assert_eq!(all.iter().filter(|u| u.starts_with("mcphost://tool/") && u.ends_with("/spec")).count(), 2, "{all:?}");

    // An OAuth session scoped `tables:read` sees exactly the table entries.
    let token = token_for_scope(&server, &ns, &key, "tables:read").await;
    let scoped = McpClient::with_bearer(&server.base_url, &token).rpc("resources/list", json!({})).await;
    let tables = uris(&scoped);
    assert_eq!(tables.len(), 2, "{scoped}");
    assert!(tables.iter().all(|u| u.starts_with("mcphost://table/")), "{tables:?}");

    // A keyless connection gets an empty list, not an error.
    let keyless = McpClient::new(&server.base_url).rpc("resources/list", json!({})).await;
    assert!(uris(&keyless).is_empty(), "{keyless}");
}
