//! PRD-mcphost-key-scoped-to-tools
//! AC3 (P0) — Given `host.key.mint {tools: ["aa", "nope"]}`, When it runs,
//! Then a not-found error names `nope` and no row is written; Given a sub-key
//! calling `host.key.mint`, Then `permission_denied`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn mint_names_the_missing_tool_writes_nothing_and_refuses_sub_keys() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &key);
    publish(&owner, "aa", "echo", json!({"schema": {"type": "object"}})).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.unwrap().unwrap();

    let err = owner
        .tools_call("host.key.mint", json!({"tools": ["aa", "nope"], "label": "bad"}))
        .await
        .expect_err("a missing tool must fail the mint");
    assert!(err.error_code.as_deref().is_some_and(|c| c.ends_with("not_found")), "{err:?}");
    assert!(err.message.contains("nope"), "error must name nope: {}", err.message);
    assert!(server.state.db.list_sub_keys(tenant.id).await.unwrap().is_empty(), "no row written");

    let minted = owner
        .tools_call("host.key.mint", json!({"tools": ["aa"], "label": "ci", "ttl_s": 600}))
        .await
        .expect("mint");
    let minted = extract_structured(&minted);
    let sub_key = minted["key"].as_str().expect("key returned once").to_string();
    assert!(sub_key.starts_with("mk_"));
    assert_eq!(minted["tools"], json!(["aa"]));
    assert_eq!(server.state.db.list_sub_keys(tenant.id).await.unwrap().len(), 1);

    let sub = McpClient::with_bearer(&server.base_url, &sub_key);
    let err = sub
        .tools_call("host.key.mint", json!({"tools": ["aa"], "label": "again"}))
        .await
        .expect_err("a sub-key must not mint");
    assert_eq!(err.error_code.as_deref(), Some("permission_denied"));
    assert_eq!(server.state.db.list_sub_keys(tenant.id).await.unwrap().len(), 1);
}
