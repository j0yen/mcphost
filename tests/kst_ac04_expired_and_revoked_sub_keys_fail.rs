//! PRD-mcphost-key-scoped-to-tools
//! AC4 (P0) — Given a sub-key with `ttl_s: 60`, When the clock passes
//! `expires_at`, Then its next call fails `key_expired`; Given
//! `host.key.revoke {label}`, Then the next call fails `key_revoked` within
//! one request.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn expired_sub_key_fails_key_expired_and_revoked_fails_key_revoked() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    let qualified = publish(&owner, "aa", "echo", json!({"schema": {"type": "object"}})).await;

    let mint = |label: &'static str| {
        let owner = &owner;
        async move {
            let r = owner
                .tools_call("host.key.mint", json!({"tools": ["aa"], "label": label, "ttl_s": 60}))
                .await
                .expect("mint");
            extract_structured(&r)["key"].as_str().unwrap().to_string()
        }
    };

    // Expiry: the clock is not injectable, so move `expires_at` into the past.
    let short = mint("short").await;
    let short_client = McpClient::with_bearer(&server.base_url, &short);
    short_client.tools_call(&qualified, json!({})).await.expect("live sub-key works");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute("UPDATE tenant_keys SET expires_at = strftime('%s','now') - 1 WHERE label = 'short'", [])
        .unwrap();
    let err = short_client.tools_call(&qualified, json!({})).await.expect_err("expired");
    assert_eq!(err.error_code.as_deref(), Some("key_expired"));
    assert_eq!(err.code, -32602);

    // Revocation takes effect on the very next request.
    let revocable = mint("revocable").await;
    let rev_client = McpClient::with_bearer(&server.base_url, &revocable);
    rev_client.tools_call(&qualified, json!({})).await.expect("live before revoke");
    owner.tools_call("host.key.revoke", json!({"label": "revocable"})).await.expect("revoke");
    let err = rev_client.tools_call(&qualified, json!({})).await.expect_err("revoked");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"));
    assert_eq!(err.code, -32602);
    let _ = ns;

    // A sub-key cannot revoke, and the primary key is unaffected.
    let err = short_client
        .tools_call("host.key.revoke", json!({"label": "revocable"}))
        .await
        .expect_err("sub-key may not revoke");
    assert!(err.error_code.is_some());
    owner.tools_call(&qualified, json!({})).await.expect("primary key still works");
}
