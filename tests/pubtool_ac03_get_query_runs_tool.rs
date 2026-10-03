//! PRD-mcphost-public-tool-url
//! AC3 (P0) — Given that URL, When `GET <url>?text=hi&n=3` runs, Then the
//! tool receives `{text: "hi", n: 3}` and the response is 200.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn get_query_string_is_coerced_into_tool_args() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echo",
                "kind": "echo",
                "spec": {
                    "schema": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}, "n": {"type": "integer"}},
                        "required": ["text", "n"],
                    },
                },
            }),
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
    let resp = http
        .get(format!("{url}?text=hi&n=3"))
        .send()
        .await
        .expect("GET /x/<token>/echo?text=hi&n=3");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["result"], json!({"text": "hi", "n": 3}), "tool must receive {{text, n}} coerced by type: {body:?}");
}
