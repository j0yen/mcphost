//! PRD-mcphost-key-delegate-child-keys
//! AC5 (P0) — Given `c1` with live children, When `host.key.revoke {label:
//! "c1"}` runs without `cascade`, Then `has_children` with `data.count == 2`
//! and nothing changes.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn dkeyc_ac05_plain_revoke_of_a_key_with_live_children_changes_nothing() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Has Children Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let c1 = delegate(&server.base_url, &primary, "c1", 3000).await;
    let c2 = delegate(&server.base_url, &c1, "c2", 2000).await;
    let c3 = delegate(&server.base_url, &c2, "c3", 1000).await;

    let err = primary
        .tools_call("host.key.revoke", json!({"label": "c1"}))
        .await
        .expect_err("live children block a plain revoke");
    assert_eq!(err.error_code.as_deref(), Some("has_children"), "{err:?}");
    assert_eq!(err.data["count"], json!(2), "{err:?}");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let revoked: i64 = conn
        .query_row("SELECT COUNT(*) FROM tenant_keys WHERE revoked_at IS NOT NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(revoked, 0, "nothing changed");
    for c in [&c1, &c2, &c3] {
        c.tools_call("host.whoami", json!({})).await.expect("still live");
    }

    // A leaf has no children, so a plain revoke of it succeeds alone.
    let out = extract_structured(&primary.tools_call("host.key.revoke", json!({"label": "c3"})).await.expect("leaf"));
    assert_eq!(out["revoked"], json!(1), "{out}");
}
