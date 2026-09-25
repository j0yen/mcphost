//! PRD-mcphost-document-store
//! AC5 -- Given three documents and one deleted, When `list {since: 0}`
//! runs, Then four rows return, the deleted one with `deleted: true`;
//! `list` without `since` returns three.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn list_since_includes_deleted_list_without_since_does_not() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for name in ["one.md", "two.md", "three.md", "four.md"] {
        client
            .tools_call("host.docs.put", json!({"name": name, "content": format!("# {name}")}))
            .await
            .unwrap_or_else(|e| panic!("put {name}: {e:?}"));
    }
    client
        .tools_call("host.docs.delete", json!({"name": "four.md"}))
        .await
        .expect("delete four.md ok");

    let since = extract_structured(
        &client
            .tools_call("host.docs.list", json!({"since": 0}))
            .await
            .expect("list since 0 ok"),
    );
    let since_docs = since["documents"].as_array().expect("documents array");
    assert_eq!(since_docs.len(), 4, "list {{since: 0}} must return four rows: {since_docs:?}");
    let deleted_rows: Vec<&serde_json::Value> = since_docs
        .iter()
        .filter(|d| d["name"] == json!("four.md"))
        .collect();
    assert_eq!(deleted_rows.len(), 1, "four.md must appear exactly once: {since_docs:?}");
    assert_eq!(deleted_rows[0]["deleted"], json!(true), "four.md row: {:?}", deleted_rows[0]);

    let live = extract_structured(
        &client
            .tools_call("host.docs.list", json!({}))
            .await
            .expect("list without since ok"),
    );
    let live_docs = live["documents"].as_array().expect("documents array");
    assert_eq!(live_docs.len(), 3, "list without since must return three live rows: {live_docs:?}");
    assert!(
        live_docs.iter().all(|d| d["name"] != json!("four.md")),
        "the deleted document must not appear in the live snapshot: {live_docs:?}"
    );
}
