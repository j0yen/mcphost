//! PRD-mcphost-demo-url-per-tool
//! AC2 (P0) — Given a tool not shared by url, an unknown tool, and an
//! unknown namespace, When each is fetched, Then all three answer 404 with
//! byte-identical empty bodies.

use crate::common;
use common::{TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn unshared_unknown_tool_and_unknown_namespace_all_404_with_empty_bodies() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Demo AC2").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish (stays private)");

    let http = reqwest::Client::new();
    let mut bodies = Vec::new();
    for path in [
        format!("/d/{ns}/echo"),
        format!("/d/{ns}/no_such_tool"),
        "/d/t_nosuchns/echo".to_string(),
    ] {
        let resp = http.get(format!("{}{path}", server.base_url)).send().await.expect("GET");
        assert_eq!(resp.status(), 404, "{path}");
        bodies.push(resp.bytes().await.expect("body"));
    }
    assert!(bodies[0].is_empty(), "empty body: {:?}", bodies[0]);
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(bodies[1], bodies[2]);
}
