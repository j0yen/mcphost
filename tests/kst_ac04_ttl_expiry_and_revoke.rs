//! PRD-mcphost-key-scoped-to-tools
//! AC4 (P0) — Given a sub-key with `ttl_s: 60`, When the clock passes
//! `expires_at`, Then its next call fails `key_expired`; Given
//! `host.key.revoke {label}`, Then the next call fails `key_revoked` within
//! one request.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn subkey_fails_key_expired_after_ttl_and_key_revoked_right_after_revoke() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Subkey Expiry Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call(
            "host.tool.publish",
            json!({"name": "ta", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ta");
    let qualified = format!("{ns}.ta");

    let mint = |label: &'static str| {
        let primary = &primary;
        async move {
            let m = extract_structured(
                &primary
                    .tools_call("host.key.mint", json!({"tools": ["ta"], "ttl_s": 60, "label": label}))
                    .await
                    .expect("mint"),
            );
            assert_eq!(m["label"], json!(label));
            m["key"].as_str().expect("key").to_string()
        }
    };

    // TTL: live now, expired once the clock passes expires_at.
    let short = mint("short").await;
    let short_client = McpClient::with_bearer(&server.base_url, &short);
    short_client.tools_call(&qualified, json!({})).await.expect("live sub-key");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = conn.query_row("SELECT strftime('%s','now')", [], |r| r.get::<_, String>(0)).unwrap();
    let now: i64 = now.parse().unwrap();
    conn.execute(
        "UPDATE tenant_keys SET expires_at = ?1 WHERE label = 'short'",
        [now - 1],
    )
    .expect("move expires_at into the past");
    let err = short_client.tools_call(&qualified, json!({})).await.expect_err("expired");
    assert_eq!(err.error_code.as_deref(), Some("key_expired"), "{err:?}");
    assert_eq!(err.code, -32602);
    // The argument path expires identically.
    let err = McpClient::new(&server.base_url)
        .tools_call(&qualified, json!({"tenant_key": short}))
        .await
        .expect_err("expired via tenant_key argument");
    assert_eq!(err.error_code.as_deref(), Some("key_expired"), "{err:?}");

    // Revoke: the very next call fails.
    let other = mint("other").await;
    let other_client = McpClient::with_bearer(&server.base_url, &other);
    other_client.tools_call(&qualified, json!({})).await.expect("live before revoke");
    primary.tools_call("host.key.revoke", json!({"label": "other"})).await.expect("revoke");
    let err = other_client.tools_call(&qualified, json!({})).await.expect_err("revoked");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{err:?}");
    assert_eq!(err.code, -32602);
    let err = other_client.tools_list().await.expect_err("revoked key cannot list either");
    assert_eq!(err.error_code.as_deref(), Some("key_revoked"), "{err:?}");

    // host.key.list shows both, never a key; revoking an unknown label is not_found.
    let listed = extract_structured(&primary.tools_call("host.key.list", json!({})).await.expect("list"));
    let keys = listed["keys"].as_array().expect("keys");
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|k| k.get("key").is_none() && k.get("key_hash").is_none()));
    let other_row = keys.iter().find(|k| k["label"] == "other").expect("other row");
    assert!(other_row["revoked_at"].is_i64());
    let err = primary.tools_call("host.key.revoke", json!({"label": "nope"})).await.expect_err("unknown label");
    assert_eq!(err.error_code.as_deref(), Some("not_found"));

    // The primary key (and host.key.rotate's usage text) are unaffected.
    primary.tools_call(&qualified, json!({})).await.expect("primary still works");
    let rotated = extract_structured(&primary.tools_call("host.key.rotate", json!({})).await.expect("rotate"));
    assert!(rotated["usage"].as_str().unwrap().contains("Sub-keys"), "rotate usage must say sub-keys survive");
}
