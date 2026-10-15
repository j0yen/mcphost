//! PRD-mcphost-key-scoped-to-tools
//! AC3 (P0) — Given `host.key.mint {tools: ["a", "nope"]}`, When it runs,
//! Then `not_found` names `nope` and no row is written; Given a sub-key
//! calling `host.key.mint`, Then `permission_denied`.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn key_rows(server: &TestServer) -> i64 {
    rusqlite::Connection::open(server.data_dir.0.join("mcphost.db"))
        .expect("open raw db")
        .query_row("SELECT COUNT(*) FROM tenant_keys", [], |r| r.get(0))
        .expect("count tenant_keys")
}

#[tokio::test]
async fn mint_names_the_missing_tool_writes_no_row_and_a_subkey_cannot_mint() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Mint Validation Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call(
            "host.tool.publish",
            json!({"name": "ta", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ta");

    let err = primary
        .tools_call("host.key.mint", json!({"tools": ["ta", "nope"], "label": "bad"}))
        .await
        .expect_err("a missing tool name must refuse the mint");
    assert_eq!(err.error_code.as_deref(), Some("not_found"), "{err:?}");
    assert!(err.message.contains("nope"), "the refusal must name the first missing tool: {}", err.message);
    assert_eq!(err.data["tool"], json!("nope"));
    assert_eq!(key_rows(&server), 0, "a refused mint must write no tenant_keys row");

    // A shared-in qualified name that does not resolve is refused the same way.
    let err = primary
        .tools_call("host.key.mint", json!({"tools": ["t_deadbeef.x"]}))
        .await
        .expect_err("an unresolvable shared-in name must refuse");
    assert_eq!(err.error_code.as_deref(), Some("not_found"));
    assert_eq!(key_rows(&server), 0);

    let ok = extract_structured(
        &primary
            .tools_call("host.key.mint", json!({"tools": ["ta"], "label": "good"}))
            .await
            .expect("valid mint"),
    );
    assert_eq!(key_rows(&server), 1);

    let sub = McpClient::with_bearer(&server.base_url, ok["key"].as_str().expect("key"));
    let err = sub
        .tools_call("host.key.mint", json!({"tools": ["ta"], "label": "nested"}))
        .await
        .expect_err("a sub-key must not mint sub-keys");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"), "{err:?}");
    assert_eq!(key_rows(&server), 1, "the refused nested mint must write no row");
}
