//! PRD-mcphost-scoped-tool-share
//! AC2 (P0) -- Given a share with `scopes: ["call"]`, When the grantee lists
//! tools and calls `host.catalog.get`, Then `_meta.share_scopes == ["call"]`
//! and the catalog entry has `scopes: ["call"]` and no `spec`.

use serde_json::json;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

#[tokio::test]
async fn shsc_ac02_grantee_sees_scopes_in_tools_list_and_catalog_get() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Grantee B").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_a);
    let grantee = McpClient::with_bearer(&server.base_url, &key_b);

    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "deploy", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    owner.tools_call("host.group.create", json!({"name": "ci"})).await.expect("group create");
    owner
        .tools_call("host.group.add", json!({"name": "ci", "namespace": ns_b}))
        .await
        .expect("group add");
    owner
        .tools_call(
            "host.tool_share",
            json!({"name": "deploy", "visibility": "group", "group": "ci", "scopes": ["call"]}),
        )
        .await
        .expect("share");

    let qualified = format!("{ns_a}.deploy");
    let listed = grantee.tools_list().await.expect("tools/list");
    let entry = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(qualified))
        .unwrap_or_else(|| panic!("grantee's tools/list must show {qualified}"));
    assert_eq!(entry["_meta"]["share_scopes"], json!(["call"]));

    let got = extract_structured(
        &grantee
            .tools_call("host.catalog.get", json!({"full_name": qualified}))
            .await
            .expect("catalog.get"),
    );
    assert_eq!(got["scopes"], json!(["call"]));
    assert!(got.get("spec").is_none(), "a call-only share must not expose a spec: {got}");
}
