//! PRD-mcphost-demo-url-per-tool
//! AC6 (P0) — Given `host.tool_share(name, visibility="url")`, When it
//! returns, Then `demo_url` equals `<public_url>/d/<ns>/<name>`; Given
//! `host.tool_unshare(name)`, Then the next `GET /d/<ns>/<name>` is 404.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn share_returns_demo_url_and_unshare_makes_it_404() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC6").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("share"),
    );
    let demo_url = shared["demo_url"].as_str().expect("demo_url").to_string();
    assert_eq!(demo_url, format!("{}/d/{}/echo", server.base_url, ns));

    let http = reqwest::Client::new();
    assert_eq!(http.get(&demo_url).send().await.unwrap().status(), 200);

    client
        .tools_call("host.tool_unshare", json!({"name": "echo"}))
        .await
        .expect("unshare");
    let resp = http.get(&demo_url).send().await.unwrap();
    assert_eq!(resp.status(), 404);
    assert!(resp.bytes().await.unwrap().is_empty());
}
