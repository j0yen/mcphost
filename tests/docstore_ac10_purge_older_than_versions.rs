//! PRD-mcphost-document-store
//! AC10 -- Given a document with 5 versions, When `purge {older_than_
//! versions: 2}` runs, Then versions 1-3 are gone, 4-5 remain, and `get
//! {version: 3}` returns not found.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn purge_drops_old_versions_keeping_older_than_versions_plus_current() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Docs AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let mut id = String::new();
    for v in 1..=5 {
        let put = extract_structured(
            &client
                .tools_call("host.docs.put", json!({"name": "versioned.md", "content": format!("v{v}")}))
                .await
                .unwrap_or_else(|e| panic!("put v{v}: {e:?}")),
        );
        assert_eq!(put["version"], json!(v));
        id = put["id"].as_str().expect("id").to_string();
    }

    let purge = extract_structured(
        &client
            .tools_call("host.docs.purge", json!({"id": id, "older_than_versions": 2}))
            .await
            .expect("purge ok"),
    );
    assert_eq!(purge["kept_from_version"], json!(4), "purge result: {purge:?}");
    assert_eq!(purge["purged_versions"], json!(3), "purge result: {purge:?}");

    for v in [4, 5] {
        let get = extract_structured(
            &client
                .tools_call("host.docs.get", json!({"id": id, "version": v, "text": true}))
                .await
                .unwrap_or_else(|e| panic!("get version {v} must still exist: {e:?}")),
        );
        assert_eq!(get["text"], json!(format!("v{v}")), "version {v}: {get:?}");
    }

    let err = client
        .tools_call("host.docs.get", json!({"id": id, "version": 3}))
        .await
        .expect_err("a purged version must read as not found");
    assert_eq!(err.error_code.as_deref(), Some("docs_not_found"), "error: {err:?}");
}
