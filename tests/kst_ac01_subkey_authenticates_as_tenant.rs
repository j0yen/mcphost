//! PRD-mcphost-key-scoped-to-tools
//! AC1 (P0) — Given tenant T with tool `a` and a minted sub-key for `["ta"]`,
//! When `tools/call a` runs with the sub-key as `tenant_key` and again as
//! `Authorization: Bearer`, Then both succeed as T with `tenants.key_hash`
//! untouched, and `calls` rows carry the sub-key's `key_id`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn raw_conn(server: &TestServer) -> rusqlite::Connection {
    rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db")
}

#[tokio::test]
async fn subkey_calls_tool_as_tenant_via_argument_and_bearer_and_calls_rows_carry_key_id() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Subkey Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call(
            "host.tool.publish",
            json!({"name": "ta", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ta");
    let qualified = format!("{ns}.ta");

    let hash_before: String = raw_conn(&server)
        .query_row("SELECT key_hash FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0))
        .expect("tenant key_hash");

    let minted = extract_structured(
        &primary
            .tools_call("host.key.mint", json!({"tools": ["ta"], "ttl_s": 3600, "label": "ci"}))
            .await
            .expect("mint"),
    );
    let sub = minted["key"].as_str().expect("minted key").to_string();
    assert!(sub.starts_with("mk_"), "sub-key must carry the mk_ prefix: {sub}");
    assert_ne!(sub, key);

    // As the `tenant_key` argument, no Authorization header at all.
    let anon = McpClient::new(&server.base_url);
    anon.tools_call(&qualified, json!({"n": 1, "tenant_key": sub}))
        .await
        .expect("sub-key as tenant_key argument");
    // As `Authorization: Bearer`.
    let bearer = McpClient::with_bearer(&server.base_url, &sub);
    bearer.tools_call(&qualified, json!({"n": 2})).await.expect("sub-key as bearer");

    let conn = raw_conn(&server);
    let hash_after: String = conn
        .query_row("SELECT key_hash FROM tenants WHERE namespace = ?1", [&ns], |r| r.get(0))
        .expect("tenant key_hash after");
    assert_eq!(hash_before, hash_after, "minting/using a sub-key must not touch tenants.key_hash");

    let sub_id: i64 = conn
        .query_row(
            "SELECT k.id FROM tenant_keys k JOIN tenants t ON t.id = k.tenant_id WHERE t.namespace = ?1 AND k.label = 'ci'",
            [&ns],
            |r| r.get(0),
        )
        .expect("tenant_keys row");
    let rows: Vec<Option<i64>> = conn
        .prepare("SELECT c.key_id FROM calls c JOIN tenants t ON t.id = c.tenant_id WHERE t.namespace = ?1 AND c.tool_name = 'ta' ORDER BY c.id")
        .unwrap()
        .query_map([&ns], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, vec![Some(sub_id), Some(sub_id)], "both sub-key calls carry its key_id");

    // The primary key's own call carries none.
    primary.tools_call(&qualified, json!({"n": 3})).await.expect("primary call");
    let null_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM calls WHERE key_id IS NULL AND tool_name = 'ta'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(null_rows, 1);
}
