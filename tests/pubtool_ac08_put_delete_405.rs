//! PRD-mcphost-public-tool-url
//! AC8 (P0) — Given `PUT` or `DELETE` to the URL, When dispatched, Then
//! 405 is returned.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn put_and_delete_are_method_not_allowed() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    let http = reqwest::Client::new();
    let put_resp = http.put(&url).send().await.expect("PUT");
    assert_eq!(put_resp.status(), 405, "PUT must be 405");

    let delete_resp = http.delete(&url).send().await.expect("DELETE");
    assert_eq!(delete_resp.status(), 405, "DELETE must be 405");
}
