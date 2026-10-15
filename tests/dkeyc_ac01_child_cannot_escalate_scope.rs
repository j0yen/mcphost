//! PRD-mcphost-key-delegate-child-keys
//! AC1 (P0) — Given a depth-1 child with scopes `["tools:call"]`, When it
//! calls `host.key.delegate {scopes: ["tools:write"]}` (with `can_delegate:
//! true` on itself), Then the response is `scope_escalation` with
//! `data.excess == {kind: "scope", value: "tools:write"}` and `tenant_keys`
//! has no new row.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn key_rows(server: &TestServer) -> i64 {
    rusqlite::Connection::open(server.data_dir.0.join("mcphost.db"))
        .expect("raw db")
        .query_row("SELECT COUNT(*) FROM tenant_keys", [], |r| r.get(0))
        .expect("count")
}

#[tokio::test]
async fn dkeyc_ac01_child_asking_for_a_wider_scope_is_refused_and_writes_no_row() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Delegate Scope Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    let c1 = extract_structured(
        &primary
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": 600, "label": "c1", "can_delegate": true}),
            )
            .await
            .expect("delegate c1"),
    );
    assert_eq!(c1["depth"], json!(1), "{c1}");
    let child = McpClient::with_bearer(&server.base_url, c1["key"].as_str().expect("key"));
    let before = key_rows(&server);

    let err = child
        .tools_call("host.key.delegate", json!({"scopes": ["tools:write"], "ttl_s": 60, "label": "wide"}))
        .await
        .expect_err("escalation must be refused");
    assert_eq!(err.error_code.as_deref(), Some("scope_escalation"), "{err:?}");
    assert_eq!(err.data["excess"], json!({"kind": "scope", "value": "tools:write"}), "{err:?}");
    assert_eq!(key_rows(&server), before, "a refused delegation writes no tenant_keys row");
}
