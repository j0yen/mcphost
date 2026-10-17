//! PRD-mcphost-scoped-tool-share
//! AC5 (P0) -- Given `scopes: ["fork"]`, When the grantee calls
//! `host.tool_fork {as: "copy"}`, Then a private tool `copy` exists in its
//! tenant with `forked_from: "t_owner.tool"` and its `tools_max` count rises
//! by one; Given `scopes: ["call"]`, Then `scope_missing` names `fork`.

use serde_json::{Value, json};

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};

async fn tool_list(client: &McpClient) -> Vec<Value> {
    let out = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    out["tools"].as_array().expect("tools array").clone()
}

#[tokio::test]
async fn shsc_ac05_fork_scope_copies_and_call_scope_is_refused() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Owner A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Grantee B").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    for name in ["forkme", "callonly"] {
        owner
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
            )
            .await
            .expect("publish");
    }
    owner.tools_call("host.group.create", json!({"name": "crew"})).await.expect("group create");
    owner
        .tools_call("host.group.add", json!({"name": "crew", "namespace": ns_b}))
        .await
        .expect("group add");
    for (name, scopes) in [("forkme", json!(["fork"])), ("callonly", json!(["call"]))] {
        owner
            .tools_call(
                "host.tool_share",
                json!({"name": name, "visibility": "group", "group": "crew", "scopes": scopes}),
            )
            .await
            .expect("share");
    }

    let before = tool_list(&b).await.len();
    b.tools_call("host.tool_fork", json!({"owner": ns_a, "tool": "forkme", "as": "copy"}))
        .await
        .unwrap_or_else(|e| panic!("fork: {} {}", e.code, e.message));
    let after = tool_list(&b).await;
    assert_eq!(after.len(), before + 1, "the fork counts toward tools_max");
    let copy = after
        .iter()
        .find(|t| t["name"] == json!(format!("{ns_b}.copy")))
        .expect("private copy exists in B's tenant");
    assert_eq!(copy["forked_from"], json!(format!("{ns_a}.forkme")));
    assert_eq!(copy["visibility"], json!("private"));

    let err = b
        .tools_call("host.tool_fork", json!({"owner": ns_a, "tool": "callonly", "as": "nope"}))
        .await
        .expect_err("a call-only share must refuse fork");
    assert_eq!(err.error_code.as_deref(), Some("scope_missing"), "{}", err.message);
    assert_eq!(err.data["required"], json!("fork"));
}
