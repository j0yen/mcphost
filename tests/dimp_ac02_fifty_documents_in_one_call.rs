//! PRD-mcphost-docs-import-and-stated-boundary
//! AC2 -- Given a tenant under quota, When `host.docs.put_many` is called with
//! 50 documents mixing `content` and `content_base64`, Then `stored` is 50,
//! `results` has 50 entries in input order with `status` `stored`, and
//! `host.docs.list` returns 50 names.

use crate::common;
use crate::dimp;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn dimp_ac02_fifty_documents_land_in_one_call_in_input_order() {
    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac2").await;

    let documents = dimp::documents(50);
    let names: Vec<String> = documents.iter().map(|d| d["name"].as_str().expect("name").to_string()).collect();
    assert!(documents.iter().any(|d| d.get("content").is_some()) && documents.iter().any(|d| d.get("content_base64").is_some()));

    let reply = dimp::put_many(&client, documents, None).await;
    assert_eq!(reply["stored"], json!(50), "reply: {reply}");
    assert_eq!(reply["refused"], json!(0), "reply: {reply}");
    assert!(reply["index_lag_seconds"].is_i64(), "reply: {reply}");

    let results = reply["results"].as_array().expect("results array");
    assert_eq!(results.len(), 50);
    for (row, name) in results.iter().zip(&names) {
        assert_eq!(row["name"], json!(name), "input order: {row}");
        assert_eq!(row["status"], json!("stored"), "row: {row}");
        assert!(row["bytes"].as_i64().unwrap_or(0) > 0, "row: {row}");
    }

    let mut listed = dimp::listed_names(&client).await;
    listed.sort();
    assert_eq!(listed.len(), 50, "list: {listed:?}");
    assert_eq!(listed, names);

    // A base64 entry is stored as its decoded text.
    assert!(dimp::text_of(&client, "doc-01.md").await.contains("dimpword1"));
}
