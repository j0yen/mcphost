//! AC1 (P0) -- PRD-mcphost-resources-for-tenant-data. Given an in-process
//! server with a path-bound tenant `/u/{secret}/mcp` that owns table
//! `orders` with 3 rows, When `resources/read mcphost://table/orders` is sent
//! on that connection, Then the single content is `application/json` with 3
//! rows; Given a keyless connection that never signed up, When the same URI
//! is read, Then `-32002` is returned and the tenants table gained 0 rows.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

/// One `resources/read` POST with NO protocol version declared, so rmcp does
/// not apply its 2026-07-28 rewrite of `-32002` to `-32602` (SEP-2164); the
/// code under test is the one a pre-2026 client sees.
async fn raw_read(base_url: &str, path: &str, bearer: Option<&str>, uri: &str) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("{base_url}{path}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Method", "resources/read")
        .header("Mcp-Name", uri)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read", "params": {"uri": uri}}));
    if let Some(b) = bearer {
        req = req.header("Authorization", format!("Bearer {b}"));
    }
    req.send().await.expect("send").json().await.expect("json body")
}

/// A tenant with an `orders` table of 3 rows, plus a `/u/{secret}/mcp` path.
async fn tenant_with_orders(server: &TestServer, name: &str) -> (String, String, String) {
    let (_ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"sku": "text", "qty": "integer"}}))
        .await
        .expect("create orders");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "orders", "rows": [{"sku": "a", "qty": 1}, {"sku": "b", "qty": 2}, {"sku": "c", "qty": 3}]}),
        )
        .await
        .expect("append orders");
    let tenant = server
        .state
        .db
        .find_tenant_by_key_hash(mcphost::auth::hash_key(&key))
        .await
        .expect("lookup")
        .expect("tenant");
    let secret = mcphost::auth::generate_url_secret();
    server
        .state
        .db
        .rotate_tenant_url_secret(tenant.id, mcphost::auth::hash_key(&secret))
        .await
        .expect("mint url secret");
    (key, secret.clone(), format!("/u/{secret}/mcp"))
}

#[tokio::test]
async fn mres_ac01_path_tenant_reads_its_table_and_keyless_gets_32002() {
    let server = TestServer::start().await;
    let (_key, _secret, path) = tenant_with_orders(&server, "mres ac01").await;

    // Path-bound connection: no Authorization header at all.
    let reply = raw_read(&server.base_url, &path, None, "mcphost://table/orders").await;
    let contents = reply["result"]["contents"].as_array().unwrap_or_else(|| panic!("no contents: {reply}"));
    assert_eq!(contents.len(), 1, "{reply}");
    assert_eq!(contents[0]["mimeType"], "application/json", "{reply}");
    assert_eq!(contents[0]["uri"], "mcphost://table/orders", "{reply}");
    let body: Value = serde_json::from_str(contents[0]["text"].as_str().expect("text")).expect("json body");
    assert_eq!(body["rows"].as_array().expect("rows").len(), 3, "{body}");

    // A keyless connection that never signed up: -32002, and no tenant minted.
    let before = server.state.db.list_tenants().await.unwrap().len();
    let keyless = raw_read(&server.base_url, "/mcp", None, "mcphost://table/orders").await;
    assert_eq!(keyless["error"]["code"], -32002, "{keyless}");
    assert_eq!(keyless["error"]["data"]["code"], "resource_not_found", "{keyless}");
    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(after, before, "a keyless read must not create a tenant");

    // Same refusal for a table the path tenant does not have.
    let missing = raw_read(&server.base_url, &path, None, "mcphost://table/nope").await;
    assert_eq!(missing["error"]["code"], -32002, "{missing}");
}
