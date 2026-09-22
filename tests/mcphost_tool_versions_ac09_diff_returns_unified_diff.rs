//! PRD-mcphost-tool-versions
//! AC9 (P2) — Given two versions, When `host.tool_diff` is called, Then a
//! unified diff is returned.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn diff_between_two_versions_is_a_unified_diff() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "diffable",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}},
            }),
        )
        .await
        .expect("publish v1 should succeed");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "diffable",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]}},
            }),
        )
        .await
        .expect("publish v2 should succeed");

    let diff = client
        .tools_call("host.tool_diff", json!({"name": "diffable", "from": 1, "to": 2}))
        .await
        .expect("tool_diff should succeed");
    let result = extract_structured(&diff);
    let diff_text = result["diff"].as_str().expect("diff must be a string");

    assert!(diff_text.starts_with("--- v1\n+++ v2\n"), "{diff_text}");
    assert!(diff_text.contains("@@"), "must contain a unified-diff hunk header: {diff_text}");
    assert!(
        diff_text
            .lines()
            .any(|l| l.starts_with('-') && l.contains("\"a\":")),
        "must show the removed field: {diff_text}"
    );
    assert!(
        diff_text
            .lines()
            .any(|l| l.starts_with('+') && l.contains("\"b\":")),
        "must show the added field: {diff_text}"
    );

    // Diffing a version against itself is empty.
    let same = client
        .tools_call("host.tool_diff", json!({"name": "diffable", "from": 2, "to": 2}))
        .await
        .expect("tool_diff of a version against itself should succeed");
    assert_eq!(extract_structured(&same)["diff"], json!(""));
}
