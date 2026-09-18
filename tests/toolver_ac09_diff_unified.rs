//! AC9 — Given two versions, When `host.tool_diff` is called, Then a
//! unified diff is returned.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn diff_returns_a_unified_diff_between_two_versions() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Publisher").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(
        &client,
        "greet",
        "echo",
        json!({"schema": {"type": "object", "properties": {"a": {"type": "string"}}}}),
    )
    .await;
    publish(
        &client,
        "greet",
        "echo",
        json!({"schema": {"type": "object", "properties": {"b": {"type": "string"}}}}),
    )
    .await;

    let result = client
        .tools_call("host.tool_diff", json!({"name": "greet", "from": 1, "to": 2}))
        .await
        .expect("tool_diff");
    let structured = extract_structured(&result);
    let diff = structured["diff"].as_str().expect("diff is a string");

    assert!(diff.contains("--- v1"), "diff: {diff}");
    assert!(diff.contains("+++ v2"), "diff: {diff}");
    assert!(
        diff.lines().any(|l| l.starts_with('-') && l.contains('"') && l.contains('a')),
        "diff must show a removed line naming property 'a': {diff}"
    );
    assert!(
        diff.lines().any(|l| l.starts_with('+') && l.contains('"') && l.contains('b')),
        "diff must show an added line naming property 'b': {diff}"
    );
    assert_eq!(structured["from"], json!(1));
    assert_eq!(structured["to"], json!(2));
}
