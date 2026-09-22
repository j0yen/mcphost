//! PRD-mcphost-tool-versions
//! AC1 (P0) — Given a tool published twice, When `host.tool_history` is
//! called, Then two versions are listed, version 2 current, with distinct
//! `source_sha256`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn publish_twice_lists_two_versions_v2_current_distinct_hash() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "greeter",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}},
            }),
        )
        .await
        .expect("publish v1 should succeed");

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "greeter",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"note": {"type": "string"}}, "required": ["note"]}},
            }),
        )
        .await
        .expect("publish v2 should succeed");

    let history = client
        .tools_call("host.tool_history", json!({"name": "greeter"}))
        .await
        .expect("tool_history should succeed");
    let result = extract_structured(&history);
    let versions = result["versions"].as_array().expect("versions array");
    assert_eq!(versions.len(), 2, "two versions must be listed: {versions:?}");

    // Newest first.
    assert_eq!(versions[0]["version"], json!(2));
    assert_eq!(versions[0]["current"], json!(true));
    assert_eq!(versions[1]["version"], json!(1));
    assert_eq!(versions[1]["current"], json!(false));

    let hash1 = versions[1]["source_sha256"]
        .as_str()
        .expect("v1 source_sha256");
    let hash2 = versions[0]["source_sha256"]
        .as_str()
        .expect("v2 source_sha256");
    assert_ne!(
        hash1, hash2,
        "different specs must produce distinct source_sha256"
    );
}
