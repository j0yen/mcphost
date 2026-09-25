//! PRD-mcphost-document-store
//! AC4 -- Given a JSON document `{"a": {"b": 1}}`, When `get {text: true}`
//! runs, Then the text contains the line `a.b: 1`; given a CSV with header
//! `x,y` and row `1,2`, the text contains `x=1, y=2`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn get_text_extracts_json_leaf_paths_and_csv_rows() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.docs.put", json!({"name": "a.json", "content": "{\"a\": {\"b\": 1}}"}))
        .await
        .expect("put json ok");
    let json_get = extract_structured(
        &client
            .tools_call("host.docs.get", json!({"name": "a.json", "text": true}))
            .await
            .expect("get json ok"),
    );
    let json_text = json_get["text"].as_str().expect("text field");
    assert!(
        json_text.lines().any(|l| l == "a.b: 1"),
        "json extraction must contain the line 'a.b: 1': {json_text:?}"
    );

    client
        .tools_call("host.docs.put", json!({"name": "a.csv", "content": "x,y\n1,2\n"}))
        .await
        .expect("put csv ok");
    let csv_get = extract_structured(
        &client
            .tools_call("host.docs.get", json!({"name": "a.csv", "text": true}))
            .await
            .expect("get csv ok"),
    );
    let csv_text = csv_get["text"].as_str().expect("text field");
    assert!(
        csv_text.contains("x=1, y=2"),
        "csv extraction must contain 'x=1, y=2': {csv_text:?}"
    );
}
