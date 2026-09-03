//! AC2 — Given an unauthenticated client, When it calls `signup` with a
//! name, Then it receives a key, a namespace matching `t_[0-9a-f]{8}`, and
//! the endpoint URL, and the `tenants` table has one row with the key
//! stored only as a hash.

mod common;
use common::{TestServer, extract_structured, signup};
use mcphost::auth::hash_key;

#[tokio::test]
async fn signup_returns_key_and_namespace_and_hashes_it() {
    let server = TestServer::start().await;
    let (tenant_ns, key) = signup(&server.base_url, "Agent Smith").await;

    assert!(
        tenant_ns.starts_with("t_") && tenant_ns.len() == 10,
        "namespace must match t_[0-9a-f]{{8}}, got {tenant_ns}"
    );
    assert!(
        tenant_ns[2..].chars().all(|c| c.is_ascii_hexdigit()),
        "namespace suffix must be hex, got {tenant_ns}"
    );

    let rows = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(rows.len(), 1, "exactly one tenant row after one signup");
    let row = &rows[0];
    assert_eq!(row.namespace, tenant_ns);
    assert_eq!(
        row.key_hash,
        hash_key(&key),
        "stored key_hash must be sha256(key)"
    );
    assert_ne!(row.key_hash, key, "the raw key must never be stored");

    // The endpoint field is present and points at /mcp.
    let client = common::McpClient::new(&server.base_url);
    let result = client
        .tools_call("signup", serde_json::json!({"name": "Second Agent"}))
        .await
        .expect("second signup");
    let structured = extract_structured(&result);
    assert!(
        structured["endpoint"].as_str().unwrap().ends_with("/mcp"),
        "endpoint must be the /mcp URL, got {structured}"
    );
}
