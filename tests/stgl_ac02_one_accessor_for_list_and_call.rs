//! PRD-mcphost-shared-tools-in-grantee-list
//! AC2 (P0) — `resolve_shared_tool` and `visible_shared_tools` agree: visible
//! for group member B, not-found/absent for non-member C.

use crate::common::{McpClient, TestServer, signup};
use mcphost::sharing::{resolve_shared_tool, visible_shared_tools};
use serde_json::json;

#[tokio::test]
async fn stgl_ac02() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "STGL2 Owner").await;
    let (ns_b, _) = signup(&server.base_url, "STGL2 Member").await;
    let (ns_c, _) = signup(&server.base_url, "STGL2 Outsider").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    a.tools_call("host.group.create", json!({"name": "g"})).await.expect("group");
    a.tools_call("host.group.add", json!({"name": "g", "namespace": ns_b})).await.expect("add");
    a.tools_call(
        "host.tool_publish",
        json!({"name": "shx", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
    )
    .await
    .expect("publish");
    a.tools_call("host.tool_share", json!({"name": "shx", "visibility": "group", "group": "g"}))
        .await
        .expect("share");

    let db = &server.state.db;
    let b = db.find_tenant_by_namespace(ns_b).await.unwrap().expect("B");
    let c = db.find_tenant_by_namespace(ns_c).await.unwrap().expect("C");

    let listed_b = visible_shared_tools(&server.state, &b).await.expect("visible B");
    let resolved_b = resolve_shared_tool(&server.state, &b, &ns_a, "shx").await;
    assert!(resolved_b.is_ok());
    assert!(listed_b.iter().any(|(o, r)| o.namespace == ns_a && r.name == "shx"));

    let listed_c = visible_shared_tools(&server.state, &c).await.expect("visible C");
    let resolved_c = resolve_shared_tool(&server.state, &c, &ns_a, "shx").await;
    assert!(resolved_c.is_err());
    assert!(listed_c.is_empty());
}
