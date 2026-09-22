//! PRD-mcphost-tool-versions
//! AC7 (P1) — Given an unpinned sharer, When the owner rolls back, Then the
//! sharer's next result carries `version_changed` once and not on the
//! following call.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn spec_v(n: u32) -> serde_json::Value {
    json!({"schema": {"type": "object", "properties": {"f": {"type": "string", "enum": [format!("v{n}")]}}, "required": ["f"]}})
}

#[tokio::test]
async fn unpinned_sharer_sees_version_changed_once_after_rollback() {
    let server = TestServer::start().await;

    let (ns_a, key_a) = signup(&server.base_url, "AC7 Owner").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    for n in 1..=2 {
        client_a
            .tools_call(
                "host.tool_publish",
                json!({"name": "shared", "kind": "echo", "spec": spec_v(n)}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish v{n} should succeed: {} {}", e.code, e.message));
    }
    client_a
        .tools_call(
            "host.tool_share",
            json!({"name": "shared", "visibility": "public", "description": "AC7"}),
        )
        .await
        .expect("share should succeed");

    let (_ns_b, key_b) = signup(&server.base_url, "AC7 Caller").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.shared");

    // First-ever unpinned call: establishes the watermark, no
    // version_changed (nothing to compare against yet).
    let first = client_b
        .tools_call(&qualified, json!({"f": "v2"}))
        .await
        .expect("first unpinned call should succeed");
    assert!(
        extract_structured(&first).get("version_changed").is_none(),
        "the very first call must not carry version_changed"
    );

    client_a
        .tools_call("host.tool_rollback", json!({"name": "shared", "version": 1}))
        .await
        .expect("owner rollback to version 1 should succeed");

    let after_rollback = client_b
        .tools_call(&qualified, json!({"f": "v1"}))
        .await
        .expect("call right after rollback should succeed");
    assert_eq!(
        extract_structured(&after_rollback)["version_changed"],
        json!({"from": 2, "to": 1}),
        "the call right after a real change must carry version_changed once"
    );

    let following = client_b
        .tools_call(&qualified, json!({"f": "v1"}))
        .await
        .expect("the following call should succeed");
    assert!(
        extract_structured(&following).get("version_changed").is_none(),
        "the call after that must not repeat version_changed with no further change"
    );
}
