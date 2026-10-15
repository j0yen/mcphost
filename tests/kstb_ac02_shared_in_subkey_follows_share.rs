//! PRD-mcphost-key-scoped-to-tools-b
//! AC2 (P0) — Given tenant B shares `lookup` to T and T mints a sub-key for
//! `["<B>.lookup"]`, When the sub-key calls it, Then it succeeds and the run
//! belongs to T; When B unshares, Then the next call fails `not_found` and
//! the sub-key row stays.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn kstb_ac02_shared_in_subkey_call_belongs_to_t_and_unshare_gives_not_found() {
    let server = TestServer::start().await;
    let (ns_b, key_b) = signup(&server.base_url, "Kstb Owner B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    client_b
        .tools_call(
            "host.tool.publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("B publishes lookup");
    client_b
        .tools_call("host.tool.share", json!({"name": "lookup", "visibility": "public"}))
        .await
        .expect("B shares lookup");

    let (ns_t, key_t) = signup(&server.base_url, "Kstb Caller T").await;
    let client_t = McpClient::with_bearer(&server.base_url, &key_t);
    let qualified = format!("{ns_b}.lookup");
    let minted = extract_structured(
        &client_t
            .tools_call("host.key.mint", json!({"tools": [qualified], "ttl_s": 3600, "label": "shared"}))
            .await
            .expect("T mints a sub-key for B's shared tool"),
    );
    let sub = McpClient::with_bearer(&server.base_url, minted["key"].as_str().expect("key"));

    sub.tools_call(&qualified, json!({})).await.expect("sub-key calls the shared-in tool");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let (caller_ns, key_id): (String, Option<i64>) = conn
        .query_row(
            "SELECT ct.namespace, c.key_id FROM calls c JOIN tenants ct ON ct.id = c.caller_tenant_id \
             WHERE c.tool_name = 'lookup' ORDER BY c.id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("calls row");
    assert_eq!(caller_ns, ns_t, "the run belongs to T, not to the owner");
    assert!(key_id.is_some(), "the run carries the sub-key's id");

    client_b
        .tools_call("host.tool.unshare", json!({"name": "lookup"}))
        .await
        .expect("B unshares");
    let err = sub.tools_call(&qualified, json!({})).await.expect_err("unshared tool");
    // The host's not-found code for an unreachable tool is `tool_not_found`.
    assert!(err.error_code.as_deref().is_some_and(|c| c.ends_with("not_found")), "{err:?}");

    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM tenant_keys WHERE label = 'shared' AND revoked_at IS NULL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "the sub-key row stays");
}
