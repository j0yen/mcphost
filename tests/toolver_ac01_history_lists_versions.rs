//! AC1 — Given a tool published twice, When `host.tool_history` is
//! called, Then two versions are listed, version 2 current, with distinct
//! `source_sha256`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn history_lists_both_versions_current_and_distinct_hashes() {
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

    let history = client
        .tools_call("host.tool_history", json!({"name": "greet"}))
        .await
        .expect("tool_history");
    let versions = extract_structured(&history)["versions"]
        .as_array()
        .expect("versions array")
        .clone();
    assert_eq!(versions.len(), 2, "two publishes must yield two versions: {versions:?}");

    // Newest-first.
    assert_eq!(versions[0]["version"], json!(2));
    assert_eq!(versions[0]["current"], json!(true));
    assert_eq!(versions[1]["version"], json!(1));
    assert_eq!(versions[1]["current"], json!(false));

    let hash1 = versions[1]["source_sha256"].as_str().expect("v1 hash");
    let hash2 = versions[0]["source_sha256"].as_str().expect("v2 hash");
    assert_ne!(hash1, hash2, "distinct specs must hash distinctly");
    assert!(!hash1.is_empty() && !hash2.is_empty());
}
