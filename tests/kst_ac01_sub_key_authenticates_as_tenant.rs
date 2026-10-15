//! PRD-mcphost-key-scoped-to-tools
//! AC1 (P0) — Given tenant T with tool `a` (published as `aa`, tool names need 2+ chars) and a minted sub-key for `["aa"]`,
//! When `tools/call a` runs with the sub-key as `tenant_key` and again as
//! `Authorization: Bearer`, Then both succeed as T with `tenants.key_hash`
//! untouched, and `calls` rows carry the sub-key's `key_id`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn sub_key_calls_succeed_as_tenant_and_stamp_key_id() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    let qualified = publish(
        &owner,
        "aa",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.aa"));

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .expect("tenant row");
    let hash_before = tenant.key_hash.clone();

    let sub_key = format!("mk_{}", mcphost::auth::generate_key());
    let key_id = server
        .state
        .db
        .insert_sub_key(
            tenant.id,
            mcphost::auth::hash_key(&sub_key),
            "ci".to_string(),
            vec!["aa".to_string()],
            1,
            i64::MAX / 2,
        )
        .await
        .expect("insert sub-key");

    // As the `tenant_key` argument.
    let anon = McpClient::new(&server.base_url);
    let via_arg = anon
        .tools_call(&qualified, json!({"msg": "arg", "tenant_key": sub_key}))
        .await
        .expect("sub-key as tenant_key must succeed");
    assert_eq!(extract_structured(&via_arg)["msg"], "arg");

    // As `Authorization: Bearer`.
    let bearer = McpClient::with_bearer(&server.base_url, &sub_key);
    let via_header = bearer
        .tools_call(&qualified, json!({"msg": "hdr"}))
        .await
        .expect("sub-key as bearer must succeed");
    assert_eq!(extract_structured(&via_header)["msg"], "hdr");

    // `tenants.key_hash` is untouched.
    let after = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.key_hash, hash_before);

    // Both calls rows carry the sub-key's id; the owner's own call does not.
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let rows: Vec<Option<i64>> = conn
        .prepare("SELECT key_id FROM calls WHERE tenant_id = ?1 AND tool_name = 'aa' ORDER BY id")
        .unwrap()
        .query_map([tenant.id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, vec![Some(key_id), Some(key_id)], "calls.key_id per call");
}
