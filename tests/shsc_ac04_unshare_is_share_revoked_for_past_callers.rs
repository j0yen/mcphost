//! PRD-mcphost-scoped-tool-share
//! AC4 (P0) -- Given a grantee that called the tool today, When the owner
//! unshares and the grantee calls 1 s later, Then `share_revoked` with
//! `revoked_at`; Given a tenant that never called it, Then
//! `shared_tool_not_found` (the host's `tool_not_found` shape).

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, signup};

#[tokio::test]
async fn shsc_ac04_past_caller_gets_share_revoked_and_stranger_gets_not_found() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Caller B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Stranger C").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let c = McpClient::with_bearer(&server.base_url, &key_c);

    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    owner.tools_call("host.group.create", json!({"name": "crew"})).await.expect("group create");
    for ns in [&ns_b, &ns_c] {
        owner
            .tools_call("host.group.add", json!({"name": "crew", "namespace": ns}))
            .await
            .expect("group add");
    }
    owner
        .tools_call("host.tool_share", json!({"name": "ping", "visibility": "group", "group": "crew"}))
        .await
        .expect("share");

    let tool = format!("{ns_a}.ping");
    b.tools_call(&tool, json!({})).await.expect("B's call while shared");

    owner.tools_call("host.tool_unshare", json!({"name": "ping"})).await.expect("unshare");
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let err = b.tools_call(&tool, json!({})).await.expect_err("revoked share must refuse");
    assert_eq!(err.error_code.as_deref(), Some("share_revoked"), "{}", err.message);
    assert_eq!(err.data["tool"], json!(tool));
    assert!(err.data["revoked_at"].as_str().is_some_and(|s| !s.is_empty()), "{}", err.data);

    let err = c.tools_call(&tool, json!({})).await.expect_err("never-called tenant gets a plain miss");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{}", err.message);
}
