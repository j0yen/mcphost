//! PRD-mcphost-document-store
//! AC2 -- Given `a.md` at version 1, When `put` runs again with different
//! content, Then the same `id` returns with `version: 2` and `seq: 2`;
//! with identical content the response repeats version 2 and `seq` is
//! unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn put_bumps_version_on_change_and_no_ops_on_identical_content() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let first = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "a.md", "content": "# hi"}))
            .await
            .expect("first put ok"),
    );
    assert_eq!(first["version"], json!(1));
    assert_eq!(first["seq"], json!(1));
    let id = first["id"].as_str().expect("id").to_string();

    let second = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "a.md", "content": "# hi there"}))
            .await
            .expect("second put ok"),
    );
    assert_eq!(second["id"], json!(id), "same name must keep the same id: {second:?}");
    assert_eq!(second["version"], json!(2), "second put: {second:?}");
    assert_eq!(second["seq"], json!(2), "second put: {second:?}");

    let third = extract_structured(
        &client
            .tools_call("host.docs.put", json!({"name": "a.md", "content": "# hi there"}))
            .await
            .expect("third put (identical content) ok"),
    );
    assert_eq!(third["id"], json!(id));
    assert_eq!(third["version"], json!(2), "identical content must repeat version 2: {third:?}");
    assert_eq!(third["seq"], json!(2), "identical content must not bump seq: {third:?}");
}
